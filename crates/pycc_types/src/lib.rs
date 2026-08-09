mod class;

use pycc_diag::{Diagnostic, Span};
#[cfg(test)]
use pycc_hir::CmpOpKind;
pub use pycc_hir::Ty;
use pycc_hir::{
    BinOpKind, CompIter, FStringPart, HirClassDef, HirExpr, HirItem, HirModule, HirStmt,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Issue #118 Part 1: tracks whether a name is *definitely* assigned on
/// every path reaching this point, or only *maybe* assigned (bound on some
/// paths but not all). A `Definitely` binding is readable; a `Maybe` binding
/// is not -- reading it raises `T0041` (possibly-unbound read), matching
/// CPython's own `NameError`/`UnboundLocalError` for the same control-flow
/// shapes. The join lattice is: `Definitely` > `Maybe` > unbound. An `if`
/// with both branches binding the same name compatibly joins to `Definitely`;
/// one branch only (or a no-else `if`) joins to `Maybe`. A `while`/`for` body
/// may execute zero times, so every body-only binding joins back as `Maybe`,
/// and a `for` loop's own target variable is `Maybe` after the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BindingState {
    Definitely(Ty),
    Maybe(Ty),
}

#[derive(Debug, Default, Clone)]
pub struct Environment {
    bindings: HashMap<String, BindingState>,
    /// Names declared by a value-less `AnnAssign` (`x: int` with no `= ...`)
    /// that have not yet been definitely assigned (issue #245). Distinct
    /// from `bindings`: an entry here records a known static type with *no*
    /// runtime value, matching CPython's own "declared, not yet assigned"
    /// semantics -- a read of such a name must still raise the existing
    /// `T0021` unbound-local diagnostic, which it does, because `lookup`
    /// only ever consults `bindings`. An entry is consumed (removed) the
    /// moment a real assignment establishes a definite binding; see
    /// `check_assignment`'s declared-consult branch.
    declared: HashMap<String, Ty>,
    functions: Arc<HashMap<String, (Vec<Ty>, Ty)>>,
    /// Names whose *net* source-order top-level binding is currently a
    /// `def` (D-110). Tracked separately from `bindings` on purpose: the
    /// representation record in `bindings` must survive a `def` so that
    /// D-040's sticky-representation rule (`check_assignment`) still rejects
    /// a later incompatible value reassignment -- PR #252's round-4 review
    /// caught a version that cleared `bindings` at a `def` and thereby let
    /// `helper = 1; def helper(): ...; helper = "leaked"` reach codegen with
    /// an `int`-allocated slot stored as `str`.
    def_rebound: HashSet<String>,
    /// Issue #22: function names whose `def` has been encountered so far in
    /// top-level source order. In top-level code, a call to a function not
    /// yet in this set is a static error (matching CPython's `NameError` for
    /// call-before-`def`). Function bodies see all module functions
    /// regardless of order (Python's late binding: a function body is
    /// evaluated at call time, by which point all module-level `def`s have
    /// typically executed), so `child_for_function` seeds this set with
    /// every function name.
    defined_functions: HashSet<String>,
    /// PR-13 Task 3 (D-133/D-134): the subset of `functions` whose signature
    /// contains a `Ty::Param` (a PEP 695 generic function), keyed by its
    /// *original* (un-mangled) name and carrying the full `HirItem` its
    /// body was lowered to. `functions` alone only has room for a resolved
    /// `(Vec<Ty>, Ty)` signature, which cannot express "resolve this call
    /// site by substituting `T`, don't just check assignability against
    /// `Ty::Param` structurally" -- `infer_expr_in`'s `HirExpr::Call` arm
    /// consults this map first, before falling through to the ordinary
    /// `functions` lookup, so a call to a generic function is dispatched to
    /// `instantiate_generic_call` instead of being rejected as an ordinary
    /// argument-type mismatch against an uninstantiated `Ty::Param`.
    generics: Arc<HashMap<String, HirItem>>,
    /// D-154 (Part 1 of #375): class name -> declared shape, mirroring
    /// `generics`'s own `Arc`-wrapped shape exactly (`Environment` is
    /// `Clone`d per function via `child_for_function`, so this needs the
    /// same cheap-clone property `functions`/`generics` already rely on).
    /// Populated once, from `HirModule::class_defs`, by every `Environment`
    /// constructor this crate has (`check_with_signatures`,
    /// `concrete_function_environment`) -- see `class::bind_classes`.
    classes: Arc<HashMap<String, HirClassDef>>,
}

impl Environment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, name: &str) -> Option<Ty> {
        match self.bindings.get(name) {
            Some(BindingState::Definitely(ty)) => Some(ty.clone()),
            // A `Maybe` binding is not readable -- `lookup` returns `None` so
            // the caller's existing unbound-local / not-defined logic fires,
            // and `infer_expr_in`'s `Name` arm can distinguish it from a
            // truly-unbound name via `binding_state` (issue #118 Part 1).
            _ => None,
        }
    }

    /// Returns the type of `name` regardless of whether it is `Definitely` or
    /// `Maybe` bound (issue #118 Part 1). Used by `check_assignment`'s
    /// first-assignment-wins rule: a maybe-bound name being reassigned on the
    /// current path becomes definite, but the representation (type) from the
    /// maybe-binding must be retained.
    pub fn lookup_any(&self, name: &str) -> Option<Ty> {
        self.bindings.get(name).map(|state| match state {
            BindingState::Definitely(ty) | BindingState::Maybe(ty) => ty.clone(),
        })
    }

    /// Returns the `BindingState` of `name` (issue #118 Part 1). Used by
    /// `infer_expr_in`'s `Name` arm and `lookup_bound_name` to distinguish
    /// three states: `Some(Definitely(ty))` -> readable, `Some(Maybe(ty))` ->
    /// `T0041` possibly-unbound read, `None` -> `T0021` unbound.
    fn binding_state(&self, name: &str) -> Option<&BindingState> {
        self.bindings.get(name)
    }

    /// Records `name` as declared with `ty` by a value-less `AnnAssign`
    /// (issue #245), without binding it -- a subsequent read still raises
    /// `T0021` via `lookup`, which never consults this map. First
    /// declaration wins: a second, still-unbound declaration for the same
    /// name is validated with `is_assignable(ty, existing)` (same direction
    /// `check_assignment` uses for an ordinary reassignment) and rejected on
    /// mismatch, but the earlier entry is what stays on success -- see
    /// `check_assignment`'s comment for the worked `bool`/`int` example.
    pub fn declare(&mut self, name: String, ty: Ty) -> Result<(), Diagnostic> {
        // Already definitely assigned (e.g. `x = 1; x: int`): unchanged
        // pre-existing behavior, out of scope for issue #245 -- leave
        // `bindings` as the sole authority and never shadow it with a
        // `declared` entry that `check_assignment` would never consult
        // anyway (its declared-consult branch only fires when `lookup`
        // already returned `None`).
        if self.bindings.contains_key(&name) {
            return Ok(());
        }
        if let Some(existing) = self.declared.get(&name) {
            if !is_assignable(ty.clone(), existing.clone()) {
                return Err(Diagnostic::error(
                    "T0026",
                    format!(
                        "cannot declare `{name}: {}`, previously declared as `{name}: {}`",
                        ty.name(),
                        existing.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            return Ok(());
        }
        self.declared.insert(name, ty);
        Ok(())
    }

    fn declared_ty(&self, name: &str) -> Option<Ty> {
        self.declared.get(name).cloned()
    }

    pub fn bind(&mut self, name: String, ty: Ty) {
        // A value assignment makes the name's net binding a value again,
        // whatever `def`s came before it (D-110).
        self.def_rebound.remove(name.as_str());
        self.bindings.insert(name, BindingState::Definitely(ty));
    }

    /// Records `name` as *maybe* bound to `ty` (issue #118 Part 1) -- the name
    /// was assigned on some but not all paths reaching this point. A
    /// subsequent definite assignment on the current path upgrades it to
    /// `Definitely` (via `bind`); a read before that raises `T0041`.
    pub fn bind_maybe(&mut self, name: String, ty: Ty) {
        self.bindings.insert(name, BindingState::Maybe(ty));
    }

    pub fn bind_function(&mut self, name: String, param_tys: Vec<Ty>, return_ty: Ty) {
        Arc::make_mut(&mut self.functions).insert(name.clone(), (param_tys, return_ty));
        // Issue #22: by default, binding a function also marks it as
        // defined. This makes standalone `infer_expr` / `check_function`
        // calls work without callers needing to separately track
        // `defined_functions`. `check_with_environment` clears this set
        // before its top-level source-order pass so call-before-`def` is
        // caught there.
        self.defined_functions.insert(name);
    }

    pub fn lookup_function(&self, name: &str) -> Option<&(Vec<Ty>, Ty)> {
        self.functions.get(name)
    }

    /// Registers `name` as a PEP 695 generic function whose original,
    /// un-substituted body is `item` (D-133/D-134). Call sites resolve
    /// through [`Self::lookup_generic`], not `lookup_function`, since a
    /// generic function's call-site behavior (substitute `T`, then
    /// type-check) cannot be expressed by a plain `(Vec<Ty>, Ty)` signature.
    pub fn bind_generic(&mut self, name: String, item: HirItem) {
        Arc::make_mut(&mut self.generics).insert(name, item);
    }

    /// Looks up `name`'s original generic-function `HirItem`, if `name` was
    /// registered via [`Self::bind_generic`].
    pub fn lookup_generic(&self, name: &str) -> Option<&HirItem> {
        self.generics.get(name)
    }

    /// Registers `name` as a declared class with shape `def` (D-154, Part 1
    /// of #375), mirroring [`Self::bind_generic`]'s own shape exactly.
    pub fn bind_class(&mut self, name: String, def: HirClassDef) {
        Arc::make_mut(&mut self.classes).insert(name, def);
    }

    /// Looks up `name`'s declared class shape, if `name` was registered via
    /// [`Self::bind_class`].
    pub fn lookup_class(&self, name: &str) -> Option<&HirClassDef> {
        self.classes.get(name)
    }

    fn child_for_function(&self, local_names: &[&str]) -> Self {
        let mut child = self.clone();
        for name in local_names {
            child.bindings.remove(*name);
            child.declared.remove(*name);
        }
        // Issue #22: a function body may call any module-level function
        // regardless of source order -- Python's late binding evaluates a
        // function body at call time, by which point all module-level
        // `def`s have typically executed. Seed `defined_functions` with
        // every known function name so the call-before-`def` check in
        // `infer_expr_in`'s `HirExpr::Call` arm never fires inside a
        // function body.
        child.defined_functions.extend(child.functions.keys().cloned());
        child
    }
}

fn unbound_local(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("local name `{name}` is not bound before this use"),
        Span::new(0, 0),
    )
}

/// Issue #118 Part 1: a name that is *maybe* bound (assigned on some but not
/// all paths reaching this use) is not safely readable. CPython raises
/// `NameError`/`UnboundLocalError` for the same control-flow shapes; the
/// strict AOT frontend rejects the read with `T0041` instead, distinguishing
/// "possibly unbound" from "never bound" (`T0021`) for actionable diagnostics.
fn possibly_unbound(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0041",
        format!("local name `{name}` may not be bound on every path reaching this use"),
        Span::new(0, 0),
    )
}

fn non_callable_binding(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("name `{name}` is bound to a non-callable value"),
        Span::new(0, 0),
    )
}

/// D-136: resolves a `HirExpr::Call`/`HirExpr::Name` callee/name string
/// that `pycc_hir`'s lowering already qualified as `<module>.<symbol>`
/// (e.g. `"math.sqrt"`, `"math.pi"`) back into its `pycc_std` registry
/// entry. A real Python identifier can never contain `.`, so `split_once`
/// finding one is itself sufficient evidence this is a stdlib-qualified
/// name, not an ordinary user identifier -- no additional import-binding
/// lookup is needed here (pycc_hir already gated construction of this
/// shape on a successful `pycc_std::resolve_module`/`resolve_symbol`
/// lookup at lowering time; re-resolving here is cheap and idempotent).
fn std_qualified_symbol(name: &str) -> Option<pycc_std::StdSymbol> {
    let (module_name, symbol_name) = name.split_once('.')?;
    let module = pycc_std::resolve_module(module_name)?;
    pycc_std::resolve_symbol(module, symbol_name)
}

fn std_scalar_to_ty(kind: pycc_std::ScalarKind) -> Ty {
    match kind {
        pycc_std::ScalarKind::Float => Ty::Float,
    }
}

/// The module-prefix portion of a `pycc_hir`-qualified stdlib name
/// (`"math"` from `"math.sqrt"`), used only for the shadowing check
/// `std_receiver_is_shadowed` performs -- kept separate from
/// `std_qualified_symbol` above so a caller can check shadowing before
/// (not after) treating the name as a confirmed stdlib reference.
fn std_receiver_name(qualified_name: &str) -> &str {
    qualified_name
        .split_once('.')
        .map_or(qualified_name, |(receiver, _)| receiver)
}

/// D-136 (post-review finding): `pycc_hir::lower_expr` resolves
/// `math.sqrt`/`math.pi` textually against the receiver's bare source
/// name (`"math"`), with no visibility into whether that name is actually
/// shadowed by a real local binding at the same call site (e.g. `def
/// f(math: float) -> float: return math.sqrt(math)` -- a legal Python
/// parameter named `math`). Real CPython would raise `AttributeError`
/// there (`float` has no `.sqrt` attribute), not silently call libm's
/// `sqrt`. `pycc_types` is the first stage with real binding-scope
/// information (`env`/`local_names`), so this check happens here rather
/// than in `pycc_hir` -- mirroring `float`'s own existing
/// user-definition-takes-priority guard (`env.lookup_function(callee).is_none()`
/// in `infer_expr_in`, `!signatures.contains_key(callee)` in
/// `collect_expr_constraints`), which solves the same class of problem
/// (a hand-recognized name colliding with a real user binding) for a
/// different hand-recognized name.
fn std_receiver_shadowed(qualified_name: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "`{}` is a local name here, not the stdlib `{}` module -- attribute access on a \
             non-module value is not supported yet",
            std_receiver_name(qualified_name),
            std_receiver_name(qualified_name)
        ),
        Span::new(0, 0),
    )
}

/// A stdlib function symbol (e.g. `math.sqrt`) referenced without a call
/// (`print(math.sqrt)`, not `math.sqrt(x)`) has no callable `Ty` this
/// compiler's type system can express -- there is no first-class function
/// type here, matching `non_callable_binding`'s own "this compiler's
/// value types are all primitives" precedent (D-110's doc comment above).
fn std_function_used_as_a_value(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("`{name}` is a stdlib function and must be called, e.g. `{name}(...)`"),
        Span::new(0, 0),
    ).with_help(format!("call it: `{name}(...)`"))
}

/// A stdlib constant (e.g. `math.pi`) called like a function
/// (`math.pi()`) -- `pycc_hir`'s lowering does not distinguish
/// `StdSymbolKind::Function` from `StdSymbolKind::Constant` at the
/// call-position (it resolves any registered symbol name into
/// `HirExpr::Call`), so this mismatch surfaces here instead.
fn std_constant_is_not_callable(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("`{name}` is a stdlib constant, not a function, and cannot be called"),
        Span::new(0, 0),
    )
}

/// Issue #142: the sorted set of known Python 3.14 callable builtin names
/// that this compiler version does not implement. These are valid Python --
/// `ValueError("x")`, `Exception("msg")`, `int("5")`, `range(10)` (as a
/// standalone call, not a `for`-loop iterable) -- but the current pycc slice
/// only hand-recognizes `print`, `len`, and `float`. A bare-name call to any
/// name in this table is a *capability gap* (`C0001`), not a name-resolution
/// failure (`T0021`): the builtin genuinely exists in Python 3.14, this
/// compiler just does not implement it yet.
///
/// The table is the 139 callable, non-dunder, non-`site`-module names in
/// Python 3.14's `builtins` module, minus the three already implemented
/// (`print`, `len`, `float`), plus `__import__` (the one callable dunder
/// that users legitimately call), yielding 137. `range` is included
/// because it only works inside `for` loops (via `HirStmt::ForRange`), not
/// as a standalone call. `site`-module additions (`copyright`, `credits`,
/// `exit`, `help`, `license`, `quit`) are excluded because they are not
/// part of the `builtins` module proper and are host-dependent. Other
/// dunders (`__build_class__`, `__debug__`, etc.) are excluded as
/// internal/interpreter-implementation details users do not call directly.
///
/// **Invariant:** the array is kept sorted lexicographically by Rust's `str`
/// ordering (byte-wise UTF-8, which for ASCII identifiers matches ASCII
/// code-point order) so `is_known_callable_builtin` can use binary search.
/// A unit test (`known_callable_builtins_table_is_sorted`) asserts this.
const KNOWN_CALLABLE_BUILTINS: &[&str] = &[
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BaseException",
    "BaseExceptionGroup",
    "BlockingIOError",
    "BrokenPipeError",
    "BufferError",
    "BytesWarning",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "DeprecationWarning",
    "EOFError",
    "EncodingWarning",
    "EnvironmentError",
    "Exception",
    "ExceptionGroup",
    "FileExistsError",
    "FileNotFoundError",
    "FloatingPointError",
    "FutureWarning",
    "GeneratorExit",
    "IOError",
    "ImportError",
    "ImportWarning",
    "IndentationError",
    "IndexError",
    "InterruptedError",
    "IsADirectoryError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "ModuleNotFoundError",
    "NameError",
    "NotADirectoryError",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PendingDeprecationWarning",
    "PermissionError",
    "ProcessLookupError",
    "PythonFinalizationError",
    "RecursionError",
    "ReferenceError",
    "ResourceWarning",
    "RuntimeError",
    "RuntimeWarning",
    "StopAsyncIteration",
    "StopIteration",
    "SyntaxError",
    "SyntaxWarning",
    "SystemError",
    "SystemExit",
    "TabError",
    "TimeoutError",
    "TypeError",
    "UnboundLocalError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeError",
    "UnicodeTranslateError",
    "UnicodeWarning",
    "UserWarning",
    "ValueError",
    "Warning",
    "ZeroDivisionError",
    "__import__",
    "abs",
    "aiter",
    "all",
    "anext",
    "any",
    "ascii",
    "bin",
    "bool",
    "breakpoint",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "delattr",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "filter",
    "format",
    "frozenset",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "list",
    "locals",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "property",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
];

/// Issue #142: returns `true` if `name` is a known Python 3.14 callable
/// builtin that this compiler version does not implement. Uses binary search
/// over the sorted [`KNOWN_CALLABLE_BUILTINS`] table. Called only after
/// user-defined function lookup (and the `print`/`len`/`float`/stdlib/class
/// special cases) have all missed, so a user `def ValueError(...)` always
/// takes priority over this classification.
fn is_known_callable_builtin(name: &str) -> bool {
    KNOWN_CALLABLE_BUILTINS.binary_search(&name).is_ok()
}

/// Issue #142: the `C0001` diagnostic for a call to a known but unsupported
/// callable builtin (e.g. `ValueError("x")`). Distinct from `T0021`'s "call
/// to undefined function" -- the builtin genuinely exists in Python 3.14,
/// this compiler just does not implement it yet.
fn unsupported_callable_builtin(name: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!("call to builtin `{name}` is valid Python but not implemented yet"),
        Span::new(0, 0),
    )
}

/// Looks up a bare name's already-bound type, producing the same
/// "unbound local" vs. "not defined" distinction `HirExpr::Name` itself
/// uses in `infer_expr_in` below. `HirStmt::ForList`'s `list` field and
/// `HirExpr::ListAppend`'s `list` field are both plain `String`s rather
/// than `HirExpr::Name` nodes (D-105's HIR shape), so they can't go
/// through `infer_expr_in`'s own `Name` arm and need this helper instead.
fn lookup_bound_name(
    env: &Environment,
    local_names: &[&str],
    name: &str,
) -> Result<Ty, Diagnostic> {
    // Issue #118 Part 1: three-way distinction -- definitely bound -> ok, maybe
    // bound -> T0041, unbound -> T0021 (local) or "not defined" (global).
    match env.binding_state(name) {
        Some(BindingState::Definitely(ty)) => Ok(ty.clone()),
        Some(BindingState::Maybe(_)) => Err(possibly_unbound(name)),
        None => {
            if is_local(local_names, name) {
                Err(unbound_local(name))
            } else {
                Err(Diagnostic::error(
                    "T0021",
                    format!("name `{name}` is not defined"),
                    Span::new(0, 0),
                ))
            }
        }
    }
}

/// True when `ty` contains a `Ty::Param` anywhere in its structure,
/// including nested inside a container (D-133/D-134). Unlike
/// `scan_signature_ty_for_param` (which additionally *rejects* a
/// container-position occurrence with `T0042`), this is a plain structural
/// predicate used only to decide whether a function needs generic
/// treatment at all -- the shape gate itself still runs via
/// `check_generic_function`/`generic_type_param_name` for a function this
/// predicate says yes to.
fn ty_contains_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::List(inner) | Ty::Set(inner) => ty_contains_param(inner),
        Ty::Dict(kv) => ty_contains_param(&kv.0) || ty_contains_param(&kv.1),
        Ty::Tuple(elems) => elems.iter().any(ty_contains_param),
        // D-154: an instance's payload is only its class's name, never a
        // `Ty::Param` (or anything else `Ty`-shaped) -- a class-typed
        // annotation isn't even resolvable yet (`annotation_to_ty` has no
        // arm for a bare class name, `pycc_hir::class`'s own doc comment),
        // so `Ty::Instance` can never carry a generic type parameter to
        // scan for.
        Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::None | Ty::Infer | Ty::Instance(_) => false,
    }
}

/// True when a function's signature makes it a PEP 695 generic function
/// (D-133/D-134) -- i.e. `check_and_resolve`/`check` must route it through
/// `check_generic_function`/`instantiate_generic_call` instead of the
/// ordinary concrete-`Ty` path.
fn is_generic_signature(params: &[(String, Ty)], return_ty: &Ty) -> bool {
    params.iter().any(|(_, ty)| ty_contains_param(ty)) || ty_contains_param(return_ty)
}

fn function_local_names<'a>(params: &'a [(String, Ty)], body: &'a [HirStmt]) -> Vec<&'a str> {
    let mut names = params
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    collect_local_names(body, &mut names);
    names
}

fn collect_local_names<'a>(body: &'a [HirStmt], names: &mut Vec<&'a str>) {
    for stmt in body {
        match stmt {
            HirStmt::Assign { target, .. } => {
                if !is_local(names, target) {
                    names.push(target);
                }
            }
            HirStmt::AnnAssign { target, .. } => {
                if !is_local(names, target) {
                    names.push(target);
                }
            }
            HirStmt::If { body, orelse, .. } => {
                collect_local_names(body, names);
                collect_local_names(orelse, names);
            }
            HirStmt::While { body, .. } => collect_local_names(body, names),
            HirStmt::ForRange { var, body, .. } => {
                if !is_local(names, var) {
                    names.push(var);
                }
                collect_local_names(body, names);
            }
            HirStmt::ForList { var, body, .. } => {
                if !is_local(names, var) {
                    names.push(var);
                }
                collect_local_names(body, names);
            }
            // PR-12 Task 3 (D-117): a comprehension introduces two new local
            // names where a plain `for` loop introduces one -- its own
            // `target` (the comprehension's result) and its synthesized
            // `var` (the loop variable, already collision-proof by
            // construction, see `pycc_hir`'s `synthesize_comp_var_name`).
            // Neither has a body to recurse into (a comprehension is not a
            // nested block of statements).
            HirStmt::ListCompAssign { target, var, .. }
            | HirStmt::SetCompAssign { target, var, .. }
            | HirStmt::DictCompAssign { target, var, .. } => {
                if !is_local(names, target) {
                    names.push(target);
                }
                if !is_local(names, var) {
                    names.push(var);
                }
            }
            // `d[k] = v` (PR-11 Task 3) reassigns an existing binding's
            // contents, not a name -- unlike `Assign`/`AnnAssign`/`ForList`
            // above, it introduces no new local name to collect.
            // `base.attr = value` (D-154) is the same shape: it mutates an
            // existing instance's attribute slot, never binds a new local
            // name.
            HirStmt::ExprStmt(_)
            | HirStmt::Return(_)
            | HirStmt::DictSet { .. }
            | HirStmt::AttrSet { .. } => {}
        }
    }
}

fn module_function_local_names(hir: &HirModule) -> Vec<Vec<&str>> {
    hir.items
        .iter()
        .map(|item| match item {
            HirItem::Function { params, body, .. } => function_local_names(params, body),
            HirItem::TopLevelStmt(_) => Vec::new(),
        })
        .collect()
}

fn is_local(local_names: &[&str], name: &str) -> bool {
    local_names.contains(&name)
}

type TypeTerm = Result<Ty, usize>;
type SignatureTerms = (Vec<String>, Vec<TypeTerm>, TypeTerm);
type BinOpConstraint = (BinOpKind, TypeTerm, TypeTerm, TypeTerm);

fn is_private_solver_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::None)
}

/// D-146 (#239): determines whether a list literal's collected element terms
/// represent a homogeneous scalar-element list this solver can carry as a
/// `Ty::List` element-type carrier. Returns the shared element `Ty` when every
/// element produced `Some(Ok(ty))`, all element types are exactly equal (exact
/// `Ty` equality, matching `infer_expr_in`'s own homogeneity rule -- NOT
/// `merge_inferred_types`, which would silently widen `bool` to `int`), and the
/// shared element type is a private-solver scalar (`is_private_solver_scalar`).
/// Returns `None` for heterogeneous lists, empty lists, non-scalar element
/// lists, or any element producing `None`/`Err` -- those keep the historical
/// `Ok(None)` behavior. The `is_private_solver_scalar` gate prevents
/// nested-container carriers (`list[list[int]]`) and non-scalar element types
/// this solver has no representation for. The returned carrier is destructured
/// by the `Subscript`/`ListPop` arms, never unified -- `unify_terms` and
/// `merge_inferred_types` are unchanged.
fn homogeneous_private_solver_scalar_list_element(element_terms: &[Option<TypeTerm>]) -> Option<Ty> {
    let first = element_terms.first()?.as_ref()?.as_ref().ok()?;
    if !is_private_solver_scalar(first) {
        return None;
    }
    if !element_terms
        .iter()
        .all(|term| matches!(term, Some(Ok(ty)) if ty == first))
    {
        return None;
    }
    Some(first.clone())
}

#[derive(Debug, PartialEq, Eq)]
struct AnnotationDefaultConstraint {
    initializer: TypeTerm,
    annotation: Ty,
}

#[derive(Debug, Default)]
struct SolverConstraints {
    binops: Vec<BinOpConstraint>,
    annotation_defaults: Vec<AnnotationDefaultConstraint>,
    non_scalar_local_terms: Vec<usize>,
}

#[derive(Debug, Clone)]
struct ConstraintEnvironment<'scope, 'hir> {
    bindings: HashMap<String, TypeTerm>,
    local_names: &'scope [&'hir str],
    /// Mirror of `Environment::def_rebound` (D-110): names whose net
    /// source-order module binding is a `def`, kept apart from the term
    /// bindings for the same reason -- terms must survive a `def` for
    /// representation purposes.
    defs_rebound: HashSet<String>,
    /// Issue #359 (Part 2 of #118): names whose binding is *maybe* —
    /// assigned in only one branch of an `if` (no `else`), or only in a
    /// loop body, or introduced as a `for` loop variable (the loop may
    /// execute zero times). Mirrors the validation pass's
    /// `BindingState::Maybe` distinction (D-147): a maybe-bound name's
    /// type term is still in `bindings` (if it IS bound, it has that
    /// type), but `collect_expr_constraints`'s `Name` arm skips
    /// unification for maybe-bound names — the validation pass's `T0041`
    /// diagnostic is the user-facing gate, not the solver's inferred type.
    maybe_bindings: HashSet<String>,
}

fn fresh_variable(parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> usize {
    let id = parents.len();
    parents.push(id);
    concrete.push(None);
    id
}

fn fresh_term(parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> TypeTerm {
    Err(fresh_variable(parents, concrete))
}

fn root(parents: &mut [usize], var: usize) -> usize {
    let parent = parents[var];
    if parent == var {
        parent
    } else {
        let root = root(parents, parent);
        parents[var] = root;
        root
    }
}

fn resolved_term(term: TypeTerm, parents: &mut [usize], concrete: &[Option<Ty>]) -> Option<Ty> {
    match term {
        Ok(ty) => Some(ty),
        Err(var) => concrete[root(parents, var)].clone(),
    }
}

fn resolved_private_signature_term(
    term: TypeTerm,
    parents: &mut [usize],
    concrete: &[Option<Ty>],
    non_scalar_local_roots: &HashSet<usize>,
) -> Option<Ty> {
    match term {
        Ok(ty) => Some(ty),
        Err(var) => {
            let term_root = root(parents, var);
            let resolved = concrete[term_root].clone()?;
            (!non_scalar_local_roots.contains(&term_root) || is_private_solver_scalar(&resolved))
                .then_some(resolved)
        }
    }
}

fn inference_conflict(code: &'static str, context: &str, left: Ty, right: Ty) -> Diagnostic {
    Diagnostic::error(
        code,
        format!(
            "{context}: conflicting inferred types `{}` and `{}`",
            left.name(),
            right.name()
        ),
        Span::new(0, 0),
    )
}

fn unify_terms(
    left: TypeTerm,
    right: TypeTerm,
    parents: &mut [usize],
    concrete: &mut [Option<Ty>],
    code: &'static str,
    context: &str,
) -> Result<bool, Diagnostic> {
    match (left, right) {
        (Ok(left), Ok(right)) => merge_inferred_types(left.clone(), right.clone())
            .map(|_| false)
            .ok_or_else(|| inference_conflict(code, context, left, right)),
        (Err(var), Ok(ty)) | (Ok(ty), Err(var)) => {
            // D-133/D-134: this constraint solver exists only to infer a
            // `Ty::Infer` parameter/return of an unannotated private
            // helper -- it has no notion of PEP 695 call-site substitution
            // the way `infer_expr_in`'s own generic-dispatch arm does. A
            // `Ty::Param` merging into a genuinely unresolved inference
            // variable here (as opposed to the `(Ok, Ok)` arm above, an
            // ordinary structural comparison -- e.g. a generic function's
            // own body unifying its parameter's term against its return
            // term, both already `Ok(Ty::Param(_))`, ordinary and correct
            // per Task 2's "opaque, self-consistent" design) would
            // otherwise silently leak the internal type-parameter name
            // into a private helper's inferred signature, surfacing later
            // as a confusing, unnamed failure (e.g. "operator Add is not
            // defined for `T` and `int`") at a span pointing at the
            // generic `def` rather than the actual call site -- this
            // project's own practice (D-105) says a precisely nameable
            // case like this should instead get a clean, specific
            // diagnostic. Reuses `T0042` (the same family of
            // "generic-function shape or call-site instantiation
            // rejected" failures Task 2 introduced) rather than inventing
            // a new code, since this is exactly that family's
            // "instantiation rejected" shape, just discovered from the
            // inference side instead of the call side.
            //
            // Checked as a plain `if` inside this arm (not a separate
            // `(Err(_), Ok(ty)) | (Ok(ty), Err(_))`-guarded arm above it)
            // deliberately: every current production call site only ever
            // reaches this function with the "known-or-generic" side in
            // one fixed position per call site (e.g. `Return`'s own
            // `unify_terms(return_term, actual)` always puts the declared
            // return term first), so a guard on a second, differently-
            // ordered `Ok`/`Err` pattern alternative would be permanently
            // unreachable dead code under the D-014 coverage gate -- this
            // single shared check covers both orderings without splitting
            // into an unreachable region.
            if ty_contains_param(&ty) {
                return Err(t0042(format!(
                    "{context} cannot be inferred through a PEP 695 generic function's own type parameter -- add an explicit type annotation instead of relying on inference here"
                )));
            }
            let root = root(parents, var);
            let merged = match concrete[root].clone() {
                Some(current) => merge_inferred_types(current.clone(), ty.clone())
                    .ok_or_else(|| inference_conflict(code, context, current, ty))?,
                None => ty,
            };
            let changed = concrete[root] != Some(merged.clone());
            concrete[root] = Some(merged);
            Ok(changed)
        }
        (Err(left), Err(right)) => {
            let left_root = root(parents, left);
            let right_root = root(parents, right);
            if left_root == right_root {
                return Ok(false);
            }
            let merged = match (concrete[left_root].clone(), concrete[right_root].clone()) {
                (Some(left), Some(right)) => Some(
                    merge_inferred_types(left.clone(), right.clone())
                        .ok_or_else(|| inference_conflict(code, context, left, right))?,
                ),
                (Some(ty), None) | (None, Some(ty)) => Some(ty),
                (None, None) => None,
            };
            parents[right_root] = left_root;
            concrete[left_root] = merged;
            Ok(true)
        }
    }
}

fn merge_inferred_types(left: Ty, right: Ty) -> Option<Ty> {
    if left == right {
        Some(left)
    } else if matches!((left, right), (Ty::Bool, Ty::Int) | (Ty::Int, Ty::Bool)) {
        Some(Ty::Int)
    } else {
        None
    }
}

fn term_for_type(ty: Ty, parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> TypeTerm {
    if ty == Ty::Infer {
        fresh_term(parents, concrete)
    } else {
        Ok(ty)
    }
}

fn collect_expr_constraints(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    binops: &mut Vec<BinOpConstraint>,
    env: &ConstraintEnvironment<'_, '_>,
    expr: &HirExpr,
) -> Result<Option<TypeTerm>, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Some(Ok(Ty::Int))),
        HirExpr::FloatLiteral(_) => Ok(Some(Ok(Ty::Float))),
        HirExpr::BoolLiteral(_) => Ok(Some(Ok(Ty::Bool))),
        HirExpr::StringLiteral(_) => Ok(Some(Ok(Ty::Str))),
        HirExpr::Name(name) => {
            // D-136: a `pycc_hir`-qualified stdlib name (`"math.pi"`) is
            // checked before ordinary binding lookup. Post-review finding:
            // the qualified string itself can never collide with a real
            // binding (see `std_qualified_symbol`'s own doc comment), but
            // its *receiver* (`"math"`) can -- a real local/parameter
            // legally named `math` shadows the stdlib module the same way
            // `float`'s own user-definition-takes-priority guard elsewhere
            // in this function handles for that hand-recognized name (see
            // `std_receiver_shadowed`'s own doc comment).
            if let Some(symbol) = std_qualified_symbol(name) {
                let receiver = std_receiver_name(name);
                if env.bindings.contains_key(receiver) || is_local(env.local_names, receiver) {
                    return Err(std_receiver_shadowed(name));
                }
                return match symbol.kind {
                    pycc_std::StdSymbolKind::Constant { ty } => {
                        Ok(Some(Ok(std_scalar_to_ty(ty))))
                    }
                    pycc_std::StdSymbolKind::Function { .. } => {
                        Err(std_function_used_as_a_value(name))
                    }
                };
            }
            // Issue #359 (Part 2 of #118): a maybe-bound name (assigned
            // in only one branch of an `if`, or only in a loop body) is
            // not safely readable — the validation pass's T0041 diagnostic
            // is the user-facing gate. In the solver, skip unification for
            // such names by returning `Ok(None)` (no type term available),
            // so the solver does not infer a return type from a value that
            // might not exist. This mirrors the validation pass's
            // `BindingState::Maybe` distinction (D-147).
            if env.maybe_bindings.contains(name.as_str()) {
                return Ok(None);
            }
            match env.bindings.get(name).cloned() {
                Some(term) => Ok(Some(term)),
                None if is_local(env.local_names, name) => Err(unbound_local(name)),
                None => Ok(None),
            }
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(expr) = part {
                    collect_expr_constraints(signatures, parents, concrete, binops, env, expr)?;
                }
            }
            Ok(Some(Ok(Ty::Str)))
        }
        HirExpr::Compare { left, right, .. } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, left)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, right)?;
            Ok(Some(Ok(Ty::Bool)))
        }
        HirExpr::BinOp { op, left, right } => {
            let left = collect_expr_constraints(signatures, parents, concrete, binops, env, left)?;
            let right =
                collect_expr_constraints(signatures, parents, concrete, binops, env, right)?;
            match (left, right) {
                (Some(left), Some(right)) => {
                    let result = fresh_term(parents, concrete);
                    binops.push((*op, left, right, result.clone()));
                    Ok(Some(result))
                }
                _ => Ok(None),
            }
        }
        HirExpr::Call { callee, args } => {
            // Mirror of `infer_expr_in`'s D-110 call-target rule (#133): an
            // active value binding shadows builtin and function lookup. At
            // module level `local_names` is empty and `bindings` holds the
            // accumulated top-level assignments in source order; for a
            // private-helper body the environment is seeded from those module
            // globals with the helper's own local names stripped and its
            // parameters re-inserted, so this gate sees a module binding
            // exactly when Python's name resolution would. Binding-first
            // ordering preserves the local diagnostics unchanged, as in
            // `infer_expr_in`. This mirror is load-bearing on its own when
            // the bound callee is neither `print` nor any `def`: without it,
            // `signatures.get` misses, the call stays unresolved, and the
            // solver dead-ends in the misleading "cannot infer return type"
            // error *before* pass 3 ever runs. For a shadowed `print`
            // specifically, the special case below would resolve the call
            // and pass 3's own gate would still catch the shadowing later --
            // there this mirror is fail-fast defense-in-depth, not the only
            // line of defense.
            if env.bindings.contains_key(callee) && !env.defs_rebound.contains(callee) {
                return Err(non_callable_binding(callee));
            }
            if is_local(env.local_names, callee) {
                return Err(unbound_local(callee));
            }
            let mut arg_terms = Vec::with_capacity(args.len());
            for arg in args {
                arg_terms.push(collect_expr_constraints(
                    signatures, parents, concrete, binops, env, arg,
                )?);
            }
            if callee == "print" {
                return Ok(Some(Ok(Ty::None)));
            }
            if callee == "len" {
                // D-105 point 3: `len(lst)` is a hand-recognized builtin
                // call, same as `print` above, not a user-declarable
                // signature. Its own return (`Ty::Int`) never depends on
                // the list's element type, so it's always producible here
                // regardless of whether the argument's own term has
                // resolved yet -- unlike `ListLiteral`/`Subscript`/
                // `ListAppend` above, there's no homogeneity-style check
                // to defer. Only a term that is *already* a known concrete
                // type can be validated at this point in constraint
                // collection (union-find resolution hasn't run yet); an
                // unresolved argument is left to the real check pass
                // (`infer_expr_in`) below, matching this solver's existing
                // lenient-until-known pattern. PR-11 Task 3 (D-123) relaxed
                // the argument-type check below to also accept `Ty::Dict`;
                // PR-11 Task 7 (D-123) relaxes it once more to also accept
                // `Ty::Set`.
                if arg_terms.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!("`len` expects exactly 1 argument, got {}", arg_terms.len()),
                        Span::new(0, 0),
                    ).with_help("pass exactly 1 argument"));
                }
                if let Some(Ok(arg_ty)) = &arg_terms[0]
                    && !matches!(arg_ty, Ty::List(_) | Ty::Dict(_) | Ty::Set(_))
                {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `{}`",
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass a `list[T]`, `dict[K, V]`, or `set[T]` value"));
                }
                return Ok(Some(Ok(Ty::Int)));
            }
            if let Some(symbol) = std_qualified_symbol(callee) {
                // Post-review finding: see `std_receiver_shadowed`'s own
                // doc comment -- a real local/parameter named `math`
                // shadows the stdlib module.
                let receiver = std_receiver_name(callee);
                if env.bindings.contains_key(receiver) || is_local(env.local_names, receiver) {
                    return Err(std_receiver_shadowed(callee));
                }
                let pycc_std::StdSymbolKind::Function {
                    arg_tys: expected_arg_tys,
                    ret_ty,
                } = symbol.kind
                else {
                    return Err(std_constant_is_not_callable(callee));
                };
                if arg_terms.len() != expected_arg_tys.len() {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`{callee}` expects {} argument(s), got {}",
                            expected_arg_tys.len(),
                            arg_terms.len()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("pass exactly {} argument(s)", expected_arg_tys.len())));
                }
                for (term, expected) in arg_terms.iter().zip(expected_arg_tys) {
                    if let Some(Ok(arg_ty)) = term
                        && *arg_ty != std_scalar_to_ty(*expected)
                    {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "`{callee}` expects `{}`, got `{}`",
                                std_scalar_to_ty(*expected).name(),
                                arg_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("pass a `{}` value", std_scalar_to_ty(*expected).name())));
                    }
                }
                return Ok(Some(Ok(std_scalar_to_ty(ret_ty))));
            }
            if callee == "float" && !signatures.contains_key(callee) {
                // A user-defined `float` takes priority over the builtin -- see
                // `infer_expr_in`'s own identical guard and its comment for why
                // this differs from `len`/`print`, which need no such guard.
                // Mirrors the `len` arm immediately above for the same reason (D-105
                // point 3's rationale applies identically): `float`'s own return type
                // (`Ty::Float`) never depends on the argument's resolved type, so it
                // is always producible here regardless of whether the argument's own
                // term has resolved yet. Only an already-concretely-resolved argument
                // can be validated at this point (union-find resolution hasn't run);
                // an unresolved argument is left to the real check pass
                // (`infer_expr_in`) above, matching this solver's existing
                // lenient-until-known pattern.
                if arg_terms.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`float` expects exactly 1 argument, got {}",
                            arg_terms.len()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass exactly 1 argument"));
                }
                if let Some(Ok(arg_ty)) = &arg_terms[0]
                    && !matches!(arg_ty, Ty::Int | Ty::Float | Ty::Bool)
                {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`float` expects an `int`, `float`, or `bool` argument, got `{}`",
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass an `int`, `float`, or `bool` value"));
                }
                return Ok(Some(Ok(Ty::Float)));
            }
            let Some(signature) = signatures.get(callee) else {
                // Issue #142: a private helper calling a known callable
                // builtin (e.g. `ValueError("x")`) gets the same `C0001`
                // classification as the final validation pass, rather than
                // deferring with `Ok(None)` -- the builtin genuinely exists
                // in Python 3.14, so it is a capability gap, not an
                // unresolved callee. A genuinely unknown name still returns
                // `Ok(None)` and defers to final validation's `T0021`.
                if is_known_callable_builtin(callee) {
                    return Err(unsupported_callable_builtin(callee));
                }
                return Ok(None);
            };
            for (index, (arg, parameter)) in arg_terms.into_iter().zip(&signature.1).enumerate() {
                // Unify whenever either side is still an inference variable --
                // not just when the callee's own parameter is unresolved.
                // This used to only match `parameter: Err(_)`, so a concrete
                // (e.g. explicitly annotated) callee parameter never
                // constrained an unresolved *caller* argument variable in the
                // reverse direction, even though `unify_terms` itself already
                // handles that case symmetrically (self-review finding,
                // pre-merge).
                if let Some(arg) = arg
                    && matches!((&arg, parameter), (Err(_), _) | (_, Err(_)))
                {
                    // Defense in depth against a `Ty::Param` leak (finding
                    // #2, PR-13 fix round): this solver has no notion of
                    // `instantiate_generic_call` -- it constrains an
                    // unannotated parameter's fresh inference variable
                    // directly against whichever concrete side is
                    // available. When that "concrete" side is actually a
                    // still-generic function's own uninstantiated `x: T`
                    // parameter type, or a still-generic call's `-> T`
                    // return type flowing in as an argument, unifying it
                    // in would bind the unannotated parameter's resolved
                    // type to the raw internal `Ty::Param` representation
                    // -- which then surfaces to the user in a later
                    // diagnostic (e.g. an `Add`-not-defined error
                    // mentioning `T` instead of a real type). Reject this
                    // shape here, before that leak can happen, with a
                    // clear, dedicated diagnostic instead.
                    if matches!(&arg, Ok(ty) if ty_contains_param(ty))
                        || matches!(parameter, Ok(ty) if ty_contains_param(ty))
                    {
                        return Err(Diagnostic::error(
                            "T0042",
                            format!(
                                "cannot infer the type of argument {} of private helper `{callee}` from a generic function's uninstantiated type; add an explicit type annotation",
                                index + 1
                            ),
                            Span::new(0, 0),
                        ).with_help("add an explicit type annotation"));
                    }
                    unify_terms(
                        parameter.clone(),
                        arg,
                        parents,
                        concrete,
                        "T0021",
                        &format!("argument {} of private helper `{callee}`", index + 1),
                    )?;
                }
            }
            Ok(Some(signature.2.clone()))
        }
        // D-146 (#239): `TypeTerm` (`Result<Ty, usize>`) has no unification-
        // friendly representation for `Ty::List` -- this solver only exists to
        // infer scalar `Ty::Infer` parameters/returns of underscore-prefixed
        // private helpers (D-045), and list homogeneity/element-type checking
        // is `infer_expr_in`'s job, not this constraint collector's (see that
        // function's `HirExpr::ListLiteral`/`Subscript`/`ListAppend` arms
        // below). Recurse into every element to keep propagating genuine
        // errors (e.g. an unbound local used as a list element). When every
        // element produces `Some(Ok(ty))`, all element types are exactly equal
        // (exact `Ty` equality, matching `infer_expr_in`'s own homogeneity
        // rule -- NOT `merge_inferred_types`, which would silently widen
        // `bool` to `int`), and the shared element type is a private-solver
        // scalar (`is_private_solver_scalar` -- `Ty::Int`/`Ty::Float`/
        // `Ty::Bool`/`Ty::Str`/`Ty::None`), return `Some(Ok(Ty::List(...)))`
        // as a destructured element-type carrier -- never unified, only
        // destructured by the `Subscript`/`ListPop` arms below to extract the
        // scalar element type for a scalar return-type inference. The
        // `is_private_solver_scalar` gate prevents nested-container carriers
        // (`list[list[int]]`) and non-scalar element types this solver has no
        // representation for. Heterogeneous lists, empty lists, non-scalar
        // element lists, or any element producing `None`/`Err` keep the
        // historical `Ok(None)` behavior -- returning `Err` here for a case
        // this solver can't actually validate would wrongly preempt
        // `checked_function_signatures`' fallback to the real, list-aware
        // check pass (`check_with_signatures`) that runs after this solver.
        // `unify_terms` and `merge_inferred_types` are unchanged -- the
        // carrier is destructured, never unified.
        HirExpr::ListLiteral(elements) => {
            let mut element_terms = Vec::with_capacity(elements.len());
            for element in elements {
                element_terms.push(collect_expr_constraints(
                    signatures, parents, concrete, binops, env, element,
                )?);
            }
            if let Some(element_ty) = homogeneous_private_solver_scalar_list_element(&element_terms)
            {
                Ok(Some(Ok(Ty::List(Box::new(element_ty)))))
            } else {
                Ok(None)
            }
        }
        HirExpr::Subscript { base, index } => {
            let base_term =
                collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, index)?;
            // D-146 (#239): when the base resolves to a `Ty::List` element-
            // type carrier (produced by the `ListLiteral` arm above or a
            // `Ty::List`-bound name), extract the scalar element type -- the
            // carrier is destructured, never unified. Otherwise keep the
            // historical `Ok(None)` behavior (the base/index recursion above
            // already propagated genuine errors).
            if let Some(Ok(Ty::List(element_ty))) = base_term {
                Ok(Some(Ok(*element_ty)))
            } else {
                Ok(None)
            }
        }
        // PR-12 Task 7 (D-118): structurally identical to `Subscript` above
        // -- a slice's base/bounds are, like a subscript's base/index,
        // ordinary sub-expressions this solver needs to keep walking into
        // (e.g. `some_param[1:3]` inside a private helper, where
        // `some_param`'s own type is exactly what the solver is trying to
        // pin down), but a `Ty::List`/`Ty::Int` base-type or bound-type gate
        // is `infer_expr_in`'s job, not this constraint collector's. Recurse
        // into `base` and every present bound only to keep propagating
        // genuine errors (e.g. an unbound local used as a bound); produce no
        // term for the `Slice` expression's own overall type.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            for bound in [start, stop, step].into_iter().flatten() {
                collect_expr_constraints(signatures, parents, concrete, binops, env, bound)?;
            }
            Ok(None)
        }
        HirExpr::ListAppend { list: _, value } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            Ok(None)
        }
        // Same reasoning as `ListLiteral` above (PR-11 Task 3): dict
        // key/value homogeneity and the `dict[str, int]`-only gate are
        // `infer_expr_in`'s job, not this solver's. Recurse into every
        // key and value only to keep propagating genuine errors.
        HirExpr::DictLiteral(pairs) => {
            for (key, value) in pairs {
                collect_expr_constraints(signatures, parents, concrete, binops, env, key)?;
                collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            }
            Ok(None)
        }
        // Same reasoning as `ListLiteral`/`DictLiteral` above (PR-11 Task
        // 7): set element homogeneity and the `set[int]`-only gate are
        // `infer_expr_in`'s job, not this solver's. Recurse into every
        // element only to keep propagating genuine errors.
        HirExpr::SetLiteral(elements) => {
            for element in elements {
                collect_expr_constraints(signatures, parents, concrete, binops, env, element)?;
            }
            Ok(None)
        }
        // Same reasoning as `ListLiteral`/`DictLiteral`/`SetLiteral` above
        // (PR-11b Task 3, D-116): the per-element int/bool/float membership
        // gate is `infer_expr_in`'s job, not this solver's. Recurse into
        // every element only to keep propagating genuine errors.
        HirExpr::TupleLiteral(elements) => {
            for element in elements {
                collect_expr_constraints(signatures, parents, concrete, binops, env, element)?;
            }
            Ok(None)
        }
        // PR-12 Task 10 (D-119): the base-type gate (`T0033`) and the
        // value/key/default-type gate (`T0021`) are `infer_expr_in`'s job,
        // not this solver's -- same reasoning as `ListAppend` above. `list`
        // is a plain `String`, not a sub-expression, so there is nothing to
        // recurse into for `ListPop`. D-146 (#239): when `list` is bound in
        // `env.bindings` to a `Ty::List` element-type carrier (produced by
        // the `ListLiteral` arm above or a `Ty::List`-typed parameter),
        // extract the scalar element type -- the carrier is destructured,
        // never unified. Otherwise keep the historical `Ok(None)` behavior.
        HirExpr::ListPop { list } => {
            if let Some(Ok(Ty::List(element_ty))) = env.bindings.get(list).cloned() {
                Ok(Some(Ok(*element_ty)))
            } else {
                Ok(None)
            }
        }
        HirExpr::DictGetOrDefault {
            dict: _,
            key,
            default,
        } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, key)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, default)?;
            Ok(None)
        }
        HirExpr::SetAdd { set: _, value } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            Ok(None)
        }
        // D-154 (Part 1 of #375): same reasoning as `Subscript`/`ListPop`
        // above -- the base-instance-type gate and attribute/method
        // resolution are `infer_expr_in`'s job, not this solver's (which
        // only ever runs for a *private, unannotated* function's body).
        // Recurse into `base` (and, for `MethodCall`, every argument) only
        // to keep propagating genuine errors; produce no term for either
        // expression's own overall type, mirroring `HirExpr::Subscript`'s
        // own pre-D-146 "no unification term" default and `ListPop`'s own
        // doc comment's documented consequence (a private function
        // assigning from one of these expressions registers no binding for
        // the target -- a pre-existing, not novel, gap this project already
        // accepts for every other container-shaped expression).
        HirExpr::AttrGet { base, .. } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            Ok(None)
        }
        HirExpr::MethodCall { base, args, .. } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            for arg in args {
                collect_expr_constraints(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(None)
        }
    }
}

/// Binds a comprehension's synthesized loop variable (PR-12 Task 3, D-117)
/// in the constraint solver's environment, mirroring `collect_block_constraints`'s
/// own `ForRange`/`ForList` arms exactly rather than duplicating their logic
/// a third time: a `CompIter::Range` iterable gives the loop variable the
/// concrete `Ty::Int` fact (unifying against any existing term, exactly like
/// `ForRange`'s own loop variable), while a `CompIter::Name` iterable gives
/// it a fresh, unconstrained term (exactly like `ForList`'s own loop
/// variable) since this solver doesn't track a list/dict/set-typed name's
/// element type at all. Shared by all three comprehension statement kinds.
fn bind_comp_loop_var(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    binops: &mut Vec<BinOpConstraint>,
    env: &mut ConstraintEnvironment<'_, '_>,
    var: &str,
    iter: &CompIter,
) -> Result<(), Diagnostic> {
    match iter {
        CompIter::Range { start, stop, step } => {
            for (position, expr) in [("start", start), ("stop", stop), ("step", step)] {
                if let Some(term @ Err(_)) =
                    collect_expr_constraints(signatures, parents, concrete, binops, env, expr)?
                {
                    unify_terms(
                        term,
                        Ok(Ty::Int),
                        parents,
                        concrete,
                        "T0021",
                        &format!("comprehension range {position}"),
                    )?;
                }
            }
            if let Some(existing) = env.bindings.get(var).cloned() {
                unify_terms(
                    existing,
                    Ok(Ty::Int),
                    parents,
                    concrete,
                    "T0023",
                    &format!("assignment to comprehension loop variable `{var}`"),
                )?;
            } else {
                env.bindings.insert(var.to_string(), Ok(Ty::Int));
            }
        }
        CompIter::Name(_) => {
            if !env.bindings.contains_key(var) {
                let term = fresh_term(parents, concrete);
                env.bindings.insert(var.to_string(), term);
            }
        }
    }
    Ok(())
}

/// Issue #359 (Part 2 of #118): joins two if-branch environments back
/// into `env` after cloning and running each branch independently.
/// Mirrors the validation pass's `join_if_branches` (D-147) but works
/// with the solver's `TypeTerm`-based `ConstraintEnvironment.bindings`
/// and tracks maybe-bound names in `maybe_bindings` instead of wrapping
/// each binding in a `BindingState` variant.
///
/// `pre_existing` is the set of binding names that were in `env.bindings`
/// before the `if` — names introduced by only one branch are maybe-bound,
/// names introduced by both branches are definitely bound.
fn join_if_branches_solver(
    env: &mut ConstraintEnvironment,
    body_env: &ConstraintEnvironment,
    orelse_env: &ConstraintEnvironment,
    pre_existing: &HashSet<String>,
) {
    // Merge bindings: first-binding-wins (body first, then orelse).
    // `entry().or_insert()` preserves the existing binding for pre-existing
    // names and takes the body's term for new names introduced by the body.
    for (name, term) in &body_env.bindings {
        env.bindings.entry(name.clone()).or_insert(term.clone());
    }
    for (name, term) in &orelse_env.bindings {
        env.bindings.entry(name.clone()).or_insert(term.clone());
    }
    // Update maybe_bindings for names introduced by the branches.
    let body_new: HashSet<&String> = body_env
        .bindings
        .keys()
        .filter(|k| !pre_existing.contains(*k))
        .collect();
    let orelse_new: HashSet<&String> = orelse_env
        .bindings
        .keys()
        .filter(|k| !pre_existing.contains(*k))
        .collect();
    for name in body_new.iter().chain(orelse_new.iter()) {
        if body_new.contains(name) && orelse_new.contains(name) {
            // Both branches bind it → definitely bound.
            env.maybe_bindings.remove(*name);
        } else {
            // Only one branch binds it → maybe bound.
            env.maybe_bindings.insert((*name).clone());
        }
    }
}

/// Issue #359 (Part 2 of #118): joins a loop body environment back into
/// `env` after cloning and running the body. Mirrors the validation pass's
/// `join_loop_body` (D-147): a loop body may execute zero times, so every
/// body-only binding is maybe-bound. Pre-existing bindings stay as-is
/// (their maybe/definite status is unchanged).
fn join_loop_body_solver(
    env: &mut ConstraintEnvironment,
    body_env: &ConstraintEnvironment,
    pre_existing: &HashSet<String>,
) {
    for (name, term) in &body_env.bindings {
        if !pre_existing.contains(name) {
            env.bindings.entry(name.clone()).or_insert(term.clone());
            env.maybe_bindings.insert(name.clone());
        }
    }
}

fn collect_block_constraints(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    constraints: &mut SolverConstraints,
    env: &mut ConstraintEnvironment<'_, '_>,
    body: &[HirStmt],
    return_term: Option<TypeTerm>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            HirStmt::Assign { target, value } => {
                // A value assignment re-shadows any earlier same-named `def`
                // (D-110), independent of the first-term-wins rule below.
                env.defs_rebound.remove(target.as_str());
                if let Some(term) = collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )? {
                    env.bindings.entry(target.clone()).or_insert(term);
                }
            }
            HirStmt::AnnAssign {
                target,
                value: Some(value),
                annotation,
            } => {
                if let Some(term) = collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )? {
                    // The annotation is a directional bound on the initializer,
                    // not a symmetric equality. Defer it until every hard
                    // call/operator constraint has been collected so later
                    // `bool` evidence is not widened to `int` merely because
                    // this body was visited first.
                    constraints
                        .annotation_defaults
                        .push(AnnotationDefaultConstraint {
                            initializer: term,
                            annotation: annotation.clone(),
                        });
                }
                // A scalar target has the declared type even when the
                // collector cannot produce an initializer term. A non-scalar
                // target is still bound, but deliberately remains unresolved:
                // otherwise a hand-built HIR module could use that annotation
                // to materialize an inferred container signature through a
                // later `return target`. Keep the existing first-binding-wins
                // representation rule; final validation checks initializer
                // compatibility and re-declarations whenever signature
                // inference can otherwise complete.
                if !env.bindings.contains_key(target) {
                    let target_term = if is_private_solver_scalar(annotation) {
                        Ok(annotation.clone())
                    } else {
                        let var = fresh_variable(parents, concrete);
                        constraints.non_scalar_local_terms.push(var);
                        Err(var)
                    };
                    env.bindings.insert(target.clone(), target_term);
                }
            }
            // Deliberately out of scope for issue #245: `ConstraintEnvironment`
            // (unlike the checker's `Environment`) has no declared-but-unbound
            // side-table, and every entry in its `env.bindings` is treated as
            // resolved and readable. Registering the annotation here the same
            // way the `Some(value)` arm above does would make the name
            // readable in this solver, silently accepting `def _f():\n    x:
            // int\n    return x` (should still raise T0021, unbound) instead
            // of only `def _f():\n    x: int\n    x = 1\n    return x`. A
            // parallel declared-side-table for `ConstraintEnvironment` is a
            // separate, independently-testable follow-up if solver-scope
            // coverage of this gap is wanted later.
            HirStmt::AnnAssign { value: None, .. } => {}
            HirStmt::ExprStmt(expr) => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    expr,
                )?;
            }
            HirStmt::If { test, body, orelse } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    test,
                )?;
                // Issue #359 (Part 2 of #118): clone the environment for
                // each branch so bindings from one branch do not leak into
                // the other, then join them back — mirroring the validation
                // pass's `join_if_branches` (D-147). Names introduced by
                // only one branch are tracked as maybe-bound in
                // `maybe_bindings`; names introduced by both branches are
                // definitely bound.
                let pre_existing: HashSet<String> =
                    env.bindings.keys().cloned().collect();
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                let mut orelse_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut orelse_env,
                    orelse,
                    return_term.clone(),
                )?;
                join_if_branches_solver(env, &body_env, &orelse_env, &pre_existing);
            }
            HirStmt::While { test, body } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    test,
                )?;
                // Issue #359 (Part 2 of #118): clone the environment for
                // the loop body so body-only bindings do not leak into the
                // post-loop environment as definitely bound. A `while` body
                // may execute zero times, so every body-only binding is
                // maybe-bound — mirroring the validation pass's
                // `join_loop_body` (D-147).
                let pre_existing: HashSet<String> =
                    env.bindings.keys().cloned().collect();
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                join_loop_body_solver(env, &body_env, &pre_existing);
            }
            HirStmt::ForRange {
                var,
                start,
                stop,
                step,
                body,
            } => {
                for (position, expr) in [("start", start), ("stop", stop), ("step", step)] {
                    if let Some(term @ Err(_)) = collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        expr,
                    )? {
                        unify_terms(
                            term,
                            Ok(Ty::Int),
                            parents,
                            concrete,
                            "T0021",
                            &format!("range {position}"),
                        )?;
                    }
                }
                // Issue #359 (Part 2 of #118): snapshot the pre-loop
                // binding names so the loop variable and body-only
                // bindings can be tracked as maybe-bound after the loop
                // (a `for` loop may execute zero times).
                let pre_existing: HashSet<String> =
                    env.bindings.keys().cloned().collect();
                if let Some(existing) = env.bindings.get(var).cloned() {
                    unify_terms(
                        existing,
                        Ok(Ty::Int),
                        parents,
                        concrete,
                        "T0023",
                        &format!("assignment to for-loop target `{var}`"),
                    )?;
                } else {
                    env.bindings.insert(var.clone(), Ok(Ty::Int));
                }
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                join_loop_body_solver(env, &body_env, &pre_existing);
                // The loop variable itself is maybe-bound after the loop
                // if it was newly introduced (the loop may not execute).
                if !pre_existing.contains(var) {
                    env.maybe_bindings.insert(var.clone());
                }
            }
            HirStmt::ForList { var, list: _, body } => {
                // Unlike `ForRange`, we have no concrete `Ty::Int` fact to
                // unify the loop variable against -- this solver doesn't
                // track a `list`-typed name's element type at all (see the
                // `HirExpr::ListLiteral`/`Subscript`/`ListAppend` arms
                // above). Give `var` a fresh, unconstrained term so a body
                // reference to it doesn't spuriously fail as "not bound"
                // (it *is* locally bound, just not solver-typed); real
                // element-type checking happens in the second, real check
                // pass (`check_with_signatures`).
                // Issue #359 (Part 2 of #118): snapshot the pre-loop binding
                // names so the loop variable and body-only bindings can be
                // tracked as maybe-bound after the loop (a `for` loop may
                // execute zero times).
                let pre_existing: HashSet<String> =
                    env.bindings.keys().cloned().collect();
                if !env.bindings.contains_key(var) {
                    let term = fresh_term(parents, concrete);
                    env.bindings.insert(var.clone(), term);
                }
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                join_loop_body_solver(env, &body_env, &pre_existing);
                // The loop variable itself is maybe-bound after the loop
                // if it was newly introduced (the loop may not execute).
                if !pre_existing.contains(var) {
                    env.maybe_bindings.insert(var.clone());
                }
            }
            HirStmt::Return(value) => {
                let Some(return_term) = return_term.clone() else {
                    continue;
                };
                let actual = match value {
                    Some(expr) => collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        expr,
                    )?,
                    None => Some(Ok(Ty::None)),
                };
                if let Some(actual) = actual {
                    unify_terms(
                        return_term,
                        actual,
                        parents,
                        concrete,
                        "T0022",
                        "private helper return type",
                    )?;
                }
            }
            // PR-11 Task 3 (D-123): `dict`'s own type isn't tracked by this
            // solver either (same reasoning as `ForList`'s `list` field
            // above) -- recurse into `key`/`value` only to keep propagating
            // genuine errors; real item-assignment type-checking is
            // `check_stmt`/`check_stmt_in_function`'s job.
            HirStmt::DictSet {
                dict: _,
                key,
                value,
            } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    key,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )?;
            }
            // D-154 (Part 1 of #375): `base`'s own type isn't tracked by
            // this solver either (same reasoning as `DictSet` above) --
            // recurse into `base`/`value` only to keep propagating genuine
            // errors; real attribute-slot type-checking is
            // `check_stmt`/`check_stmt_in_function`'s job.
            HirStmt::AttrSet { base, value, .. } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    base,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )?;
            }
            // PR-12 Task 3 (D-117): no unification term is registered for
            // `target` -- per D-116's own correction note ("a
            // container-literal assignment's target never receives a solver
            // binding at all... confirmed empirically for all four container
            // types"), a comprehension's own `target` gets that same
            // treatment, matching `ListLiteral`/`DictLiteral`/`SetLiteral`
            // above. Unlike a bare no-op, though, this arm still binds the
            // loop variable (mirroring `ForRange`/`ForList` above, via the
            // shared `bind_comp_loop_var` helper) and recurses into
            // `cond`/`elt` (or `key`/`value`) to keep propagating genuine
            // errors and, critically, to let a call inside a comprehension's
            // own sub-expressions still participate in this solver's
            // argument<->parameter unification (self-review finding: an
            // earlier no-op-only version of this arm made an unannotated
            // private-helper parameter used only inside a comprehension's
            // `elt` spuriously fail to infer with "cannot infer type of
            // parameter ...; add an annotation", since the call inside `elt`
            // was never visited at all -- see
            // `private_helper_parameter_is_inferred_through_a_comprehension_s_elt`).
            HirStmt::ListCompAssign {
                var,
                iter,
                cond,
                elt,
                ..
            }
            | HirStmt::SetCompAssign {
                var,
                iter,
                cond,
                elt,
                ..
            } => {
                bind_comp_loop_var(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    var,
                    iter,
                )?;
                if let Some(cond) = cond {
                    collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        cond,
                    )?;
                }
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    elt,
                )?;
            }
            HirStmt::DictCompAssign {
                var,
                iter,
                cond,
                key,
                value,
                ..
            } => {
                bind_comp_loop_var(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    var,
                    iter,
                )?;
                if let Some(cond) = cond {
                    collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        cond,
                    )?;
                }
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    key,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )?;
            }
        }
    }
    Ok(())
}

fn propagate_binop_constraints(
    binops: &[BinOpConstraint],
    parents: &mut [usize],
    concrete: &mut [Option<Ty>],
) -> Result<(), Diagnostic> {
    loop {
        let mut changed = false;
        for &(op, ref left_term, ref right_term, ref result_term) in binops {
            let left = resolved_term(left_term.clone(), parents, concrete);
            let right = resolved_term(right_term.clone(), parents, concrete);
            let result = resolved_term(result_term.clone(), parents, concrete);
            if let (Some(left), Some(right)) = (left, right) {
                let result_ty = numeric_result_type(op, left, right)?;
                changed |= unify_terms(
                    result_term.clone(),
                    Ok(result_ty),
                    parents,
                    concrete,
                    "T0021",
                    "binary expression",
                )?;
                continue;
            }

            // Propagate constraints backward when the result determines a
            // unique operand representation. In particular, an annotated
            // `int` result for a non-division binary expression rules out
            // floats and strings, so unresolved operands are int-like and
            // use the merged `int` representation. This makes
            // `def _inc(x) -> int: return x + 1` infer `x: int` without a
            // call-site constraint (D-045).
            if result == Some(Ty::Int) && op != BinOpKind::Div {
                let left_changed = unify_terms(
                    left_term.clone(),
                    Ok(Ty::Int),
                    parents,
                    concrete,
                    "T0021",
                    "left operand of int binary expression",
                )?;
                let right_changed = unify_terms(
                    right_term.clone(),
                    Ok(Ty::Int),
                    parents,
                    concrete,
                    "T0021",
                    "right operand of int binary expression",
                )?;
                changed |= left_changed || right_changed;
            }
        }
        if !changed {
            return Ok(());
        }
    }
}

fn apply_annotation_defaults(
    constraints: &[AnnotationDefaultConstraint],
    parents: &mut [usize],
    concrete: &mut [Option<Ty>],
) -> Result<(), Diagnostic> {
    let mut bounds_by_root = vec![Vec::new(); parents.len()];
    for constraint in constraints {
        let Err(var) = &constraint.initializer else {
            continue;
        };
        if !is_private_solver_scalar(&constraint.annotation) {
            // Private-helper inference is deliberately scalar-only. A
            // hand-built HIR module must not use an annotated local to leak a
            // container type into an otherwise-unresolved signature.
            continue;
        }
        let root = root(parents, *var);
        if concrete[root].is_none() {
            bounds_by_root[root].push(constraint.annotation.clone());
        }
    }

    for (root, bounds) in bounds_by_root.iter_mut().enumerate() {
        if bounds.is_empty() {
            continue;
        }
        bounds.sort_by_key(Ty::name);
        bounds.dedup();
        let fallback = bounds.iter().find(|candidate| {
            bounds
                .iter()
                .all(|bound| is_assignable((*candidate).clone(), bound.clone()))
        });
        let Some(fallback) = fallback else {
            let bounds = bounds
                .iter()
                .map(|bound| format!("`{}`", bound.name()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Diagnostic::error(
                "T0021",
                format!("annotated initializer has incompatible directional constraints: {bounds}"),
                Span::new(0, 0),
            ));
        };
        // This root was deliberately selected only while unresolved, and
        // defaults never union roots. Install the chosen directional fallback
        // directly instead of routing it through symmetric type merging.
        concrete[root] = Some(fallback.clone());
    }
    Ok(())
}

fn contains_return(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Return(_) => true,
        HirStmt::If { body, orelse, .. } => contains_return(body) || contains_return(orelse),
        HirStmt::While { body, .. }
        | HirStmt::ForRange { body, .. }
        | HirStmt::ForList { body, .. } => contains_return(body),
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. } => false,
    })
}

/// Issue #118 Part 1: returns true if any statement in `body` introduces
/// a new binding (assignment, annotated assignment, comprehension assignment,
/// or dict/set item assignment, including nested inside if/while/for). Used
/// to skip the expensive `env.clone()` + `join_if_branches` path when neither
/// branch of an `if` assigns anything -- the common case for guard-only ifs.
fn introduces_bindings(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. } => true,
        HirStmt::If { body, orelse, .. } => {
            introduces_bindings(body) || introduces_bindings(orelse)
        }
        HirStmt::While { body, .. } => introduces_bindings(body),
        HirStmt::ForRange { body, .. } => introduces_bindings(body),
        HirStmt::ForList { body, .. } => introduces_bindings(body),
        HirStmt::Return(_) | HirStmt::ExprStmt(_) => false,
    })
}

/// Issue #118 Part 1: fast-path helper for module-scope `if` statements
/// where neither branch introduces new bindings. Checks both branches
/// in-place without cloning env, matching the pre-#118 behavior.
fn check_if_branches_in_place(
    env: &mut Environment,
    body: &[HirStmt],
    orelse: &[HirStmt],
) -> Result<(), Diagnostic> {
    body.iter().try_for_each(|stmt| check_stmt(env, stmt))?;
    orelse.iter().try_for_each(|stmt| check_stmt(env, stmt))
}

/// Issue #118 Part 1: fast-path helper for module-scope `while` loops
/// where the body introduces no new bindings. Checks the body in-place
/// without cloning env.
fn check_while_body_in_place(
    env: &mut Environment,
    body: &[HirStmt],
) -> Result<(), Diagnostic> {
    body.iter().try_for_each(|stmt| check_stmt(env, stmt))
}

/// Issue #118 Part 1: fast-path helper for function-scope `if` statements
/// where neither branch introduces new bindings.
fn check_if_branches_in_place_in_function(
    env: &mut Environment,
    local_names: &[&str],
    body: &[HirStmt],
    orelse: &[HirStmt],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    body.iter().try_for_each(|s| check_stmt_in_function(env, local_names, s, return_ty.clone()))?;
    orelse.iter().try_for_each(|s| check_stmt_in_function(env, local_names, s, return_ty.clone()))
}

/// Issue #118 Part 1: fast-path helper for function-scope `while` loops
/// where the body introduces no new bindings.
fn check_while_body_in_place_in_function(
    env: &mut Environment,
    local_names: &[&str],
    body: &[HirStmt],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    body.iter().try_for_each(|s| check_stmt_in_function(env, local_names, s, return_ty.clone()))
}

fn concrete_function_signatures(hir: &HirModule) -> Option<HashMap<String, (Vec<Ty>, Ty)>> {
    let mut signatures = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if *return_ty == Ty::Infer || params.iter().any(|(_, ty)| *ty == Ty::Infer) {
            return None;
        }
        signatures.insert(
            name.clone(),
            (
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_ty.clone(),
            ),
        );
    }
    Some(signatures)
}

/// Builds the function registry for a fully annotated module directly from
/// HIR. Unlike [`concrete_function_signatures`] followed by
/// [`check_with_signatures`], this creates each owned name and parameter vector
/// only once. `check` does not need to materialize a second signature map for a
/// downstream consumer, so its overwhelmingly common concrete, valid path can
/// validate with this registry directly.
fn concrete_function_environment(hir: &HirModule) -> Option<Environment> {
    let mut functions = HashMap::new();
    let mut generics = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if *return_ty == Ty::Infer || params.iter().any(|(_, ty)| *ty == Ty::Infer) {
            return None;
        }
        if is_generic_signature(params, return_ty) {
            generics.insert(name.clone(), item.clone());
        }
        functions.insert(
            name.clone(),
            (
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_ty.clone(),
            ),
        );
    }
    Some(Environment {
        bindings: HashMap::new(),
        declared: HashMap::new(),
        functions: Arc::new(functions),
        def_rebound: HashSet::new(),
        defined_functions: HashSet::new(),
        generics: Arc::new(generics),
        classes: Arc::new(hir.class_defs.iter().cloned().collect()),
    })
}

fn infer_function_signatures_with_solver(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
    let mut parents = Vec::new();
    let mut concrete = Vec::new();
    let mut signatures = HashMap::new();
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        {
            signatures.insert(
                name.clone(),
                (
                    params.iter().map(|(name, _)| name.clone()).collect(),
                    params
                        .iter()
                        .map(|(_, ty)| term_for_type(ty.clone(), &mut parents, &mut concrete))
                        .collect(),
                    term_for_type(return_ty.clone(), &mut parents, &mut concrete),
                ),
            );
        }
    }

    let mut constraints = SolverConstraints::default();
    let mut globals = ConstraintEnvironment {
        bindings: HashMap::new(),
        local_names: &[],
        defs_rebound: HashSet::new(),
        maybe_bindings: HashSet::new(),
    };
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                collect_block_constraints(
                    &signatures,
                    &mut parents,
                    &mut concrete,
                    &mut constraints,
                    &mut globals,
                    std::slice::from_ref(stmt),
                    None,
                )?;
            }
            // Mirror of pass 2's source-order `def` rebinding (D-110): the
            // `def` marks the name def-rebound in the accumulated globals
            // (without erasing its term, which representation tracking may
            // still need), so helper-body environments seeded from them see
            // the net binding, not a stale shadowed primitive.
            HirItem::Function { name, .. } => {
                globals.defs_rebound.insert(name.clone());
            }
        }
    }
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        let HirItem::Function { name, body, params, .. } = item else {
            continue;
        };
        let signature = &signatures[name];
        let mut env = ConstraintEnvironment {
            bindings: globals.bindings.clone(),
            local_names,
            defs_rebound: globals.defs_rebound.clone(),
            maybe_bindings: globals.maybe_bindings.clone(),
        };
        for local_name in local_names.iter().copied() {
            env.bindings.remove(local_name);
            // A local name (parameter or body-assigned) re-binds within this
            // body, so a stale module-level def-rebound fact must not
            // survive for it (D-110, PR #252's round-6 review): a parameter
            // colliding with a def-rebound module name would otherwise skip
            // the mirror gate and be mislabeled "not bound before this use".
            env.defs_rebound.remove(local_name);
            env.maybe_bindings.remove(local_name);
        }
        // Use the current item's own parameter names, not the last-inserted
        // signature's names (#386): a redefined method shares its mangled
        // name but has its own parameter names, and checking its body against
        // the wrong names would report false T0021 "not bound" errors. The
        // type terms (signature.1) and return type (signature.2) come from
        // the last definition, which is correct — compatible redefinitions
        // have the same raw type shape (already validated by
        // check_incompatible_redefinitions), and the last definition is the
        // one bound at call sites.
        for (param_name, param_ty) in params
            .iter()
            .map(|(n, _)| n)
            .zip(&signature.1)
        {
            env.bindings.insert(param_name.clone(), param_ty.clone());
        }
        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            body,
            Some(signature.2.clone()),
        )?;
        if signature.2.is_err() && !contains_return(body) {
            unify_terms(
                signature.2.clone(),
                Ok(Ty::None),
                &mut parents,
                &mut concrete,
                "T0022",
                "private helper implicit return",
            )?;
        }
    }

    // Annotation bounds are directional defaults, not hard equalities. Let
    // every call/operator fact settle first, aggregate all remaining bounds
    // per union-find root, then propagate any selected fallback back through
    // operators. This keeps inference independent of body/declaration order.
    propagate_binop_constraints(&constraints.binops, &mut parents, &mut concrete)?;
    apply_annotation_defaults(
        &constraints.annotation_defaults,
        &mut parents,
        &mut concrete,
    )?;
    propagate_binop_constraints(&constraints.binops, &mut parents, &mut concrete)?;

    let non_scalar_local_roots = constraints
        .non_scalar_local_terms
        .iter()
        .map(|&var| root(&mut parents, var))
        .collect::<HashSet<_>>();

    let mut resolved = HashMap::new();
    for (name, signature) in &signatures {
        let param_tys = signature
            .0
            .iter()
            .zip(signature.1.iter().cloned())
            .map(|(param_name, term)| {
                resolved_private_signature_term(
                    term,
                    &mut parents,
                    &concrete,
                    &non_scalar_local_roots,
                )
                .ok_or_else(|| {
                    Diagnostic::error(
                        "T0021",
                        format!(
                            "cannot infer type of parameter `{param_name}` in private helper `{name}`; add an annotation"
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("add a type annotation to parameter `{param_name}`"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = resolved_private_signature_term(
            signature.2.clone(),
            &mut parents,
            &concrete,
            &non_scalar_local_roots,
        )
        .ok_or_else(|| {
            Diagnostic::error(
                "T0021",
                format!("cannot infer return type of private helper `{name}`; add an annotation"),
                Span::new(0, 0),
            ).with_help(format!("add a return type annotation to `{name}`"))
        })?;
        resolved.insert(name.clone(), (param_tys, return_ty));
    }
    Ok(resolved)
}

fn checked_function_signatures(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
    // Issue #22: reject incompatible redefinitions before trying either the
    // concrete or solver path (same rationale as `check`'s own call).
    check_incompatible_redefinitions(hir)?;
    // Fully annotated valid modules have no inference variables to constrain.
    // Validate them once and avoid the preceding constraint-collection walk.
    // If validation fails, deliberately fall back to the historical
    // solver-first sequence so modules with multiple errors retain the same
    // first diagnostic as before this fast path existed.
    if let Some(signatures) = concrete_function_signatures(hir)
        && check_with_signatures(hir, &signatures, function_local_names).is_ok()
    {
        return Ok(signatures);
    }

    let signatures = infer_function_signatures_with_solver(hir, function_local_names)?;
    check_with_signatures(hir, &signatures, function_local_names)?;
    Ok(signatures)
}

pub fn infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic> {
    infer_expr_in(env, &[], expr)
}

fn infer_expr_in(
    env: &Environment,
    local_names: &[&str],
    expr: &HirExpr,
) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Ty::Int),
        HirExpr::FloatLiteral(_) => Ok(Ty::Float),
        HirExpr::BoolLiteral(_) => Ok(Ty::Bool),
        HirExpr::StringLiteral(_) => Ok(Ty::Str),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(expr) = part {
                    infer_expr_in(env, local_names, expr)?; // any interpolatable type is allowed; Python str()-coerces at runtime
                }
            }
            Ok(Ty::Str)
        }
        HirExpr::Name(name) => {
            if let Some(symbol) = std_qualified_symbol(name) {
                // Post-review finding: see `std_receiver_shadowed`'s own
                // doc comment -- a real local/parameter named `math`
                // shadows the stdlib module.
                let receiver = std_receiver_name(name);
                if env.lookup(receiver).is_some() || is_local(local_names, receiver) {
                    return Err(std_receiver_shadowed(name));
                }
                return match symbol.kind {
                    pycc_std::StdSymbolKind::Constant { ty } => Ok(std_scalar_to_ty(ty)),
                    pycc_std::StdSymbolKind::Function { .. } => {
                        Err(std_function_used_as_a_value(name))
                    }
                };
            }
            // Issue #118 Part 1: three-way distinction -- definitely bound ->
            // ok, maybe bound -> T0041, unbound -> T0021 (local) or "not
            // defined" (global). The stdlib-qualified check above already
            // handled `math.sqrt`-style names.
            match env.binding_state(name) {
                Some(BindingState::Definitely(ty)) => Ok(ty.clone()),
                Some(BindingState::Maybe(_)) => Err(possibly_unbound(name)),
                None => {
                    if is_local(local_names, name) {
                        Err(unbound_local(name))
                    } else {
                        Err(Diagnostic::error(
                            "T0021",
                            format!("name `{name}` is not defined"),
                            Span::new(0, 0), // real span threading through HIR is out of scope for this task -- see Task 15's follow-up note
                        ))
                    }
                }
            }
        }
        HirExpr::BinOp { op, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            numeric_result_type(*op, left_ty, right_ty)
        }
        HirExpr::Compare { op: _, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            if numeric_or_bool_compatible(left_ty.clone(), right_ty.clone()) {
                Ok(Ty::Bool)
            } else {
                Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot compare `{}` and `{}`",
                        left_ty.name(),
                        right_ty.name()
                    ),
                    Span::new(0, 0),
                ))
            }
        }
        HirExpr::Call { callee, args } => {
            // D-110 (#133): a call target resolves through the active value
            // binding before any builtin or function-registry fallback -- a
            // module `helper = 1` shadows both a same-named `def` and a
            // builtin at every later call site, and every value binding in
            // the current subset is a primitive, so a shadowed target is
            // always non-callable. The gate is deliberately callee-first
            // (before argument inference), uniform with how the local gate
            // below always behaved. Local diagnostics are preserved exactly:
            // a value-bound local reported `non_callable_binding` before this
            // reordering too, and a local without a binding still falls
            // through to `unbound_local`. In pass 3 the environment is the
            // final module environment (D-041), so a body call is rejected
            // when the callee is value-bound anywhere at top level -- D-110
            // records that consequence (it rejects some later-rebind programs
            // CPython's dynamic order would run) as deliberate; source-order
            // visibility questions stay #22's scope.
            if env.lookup(callee).is_some() && !env.def_rebound.contains(callee) {
                return Err(non_callable_binding(callee));
            }
            // Issue #118 Part 1: a maybe-bound callee is not callable -- it
            // may not be bound on every path reaching this call. Reject with
            // T0041 before the `is_local` unbound check below, since a
            // maybe-bound local is "possibly unbound," not "never bound."
            if matches!(env.binding_state(callee), Some(BindingState::Maybe(_))) {
                return Err(possibly_unbound(callee));
            }
            if is_local(local_names, callee) {
                return Err(unbound_local(callee));
            }
            // For a callee that survives D-110's binding gate above, preserve
            // the established diagnostic order by inferring every
            // argument before validating arity or compatibility. Most Python
            // calls are small, so keep up to four inferred types on the stack
            // and reserve a heap vector only for wider calls.
            const INLINE_ARG_TYPES: usize = 4;
            let mut inline_arg_tys = [const { Ty::Infer }; INLINE_ARG_TYPES];
            let heap_arg_tys;
            let arg_tys: &[Ty] = if args.len() <= INLINE_ARG_TYPES {
                for (slot, arg) in inline_arg_tys.iter_mut().zip(args) {
                    *slot = infer_expr_in(env, local_names, arg)?;
                }
                &inline_arg_tys[..args.len()]
            } else {
                heap_arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                &heap_arg_tys
            };
            if callee == "print" {
                return Ok(Ty::None); // print's own signature isn't user-declarable in v0.1
            }
            if callee == "len" {
                // D-105 point 3: `len(lst)` is a hand-recognized builtin
                // call, same as `print` above -- not a user-declarable
                // signature. Generic over any scalar element type (`T0034`
                // already gates non-`int` lists further upstream, at the
                // point a list literal is constructed), reusing T0033 for
                // both failure shapes (wrong arity, non-list argument) --
                // the same "value does not support list operations" shape
                // already established for `ForList`/`Subscript`/`ListAppend`.
                // PR-11 Task 3 (D-123): also accepts `Ty::Dict` (gated the
                // same way by `T0036`), so `len(d)` type-checks too. PR-11
                // Task 7 (D-123): also accepts `Ty::Set` (gated the same way
                // by `T0038`), so `len(s)` type-checks too.
                if arg_tys.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!("`len` expects exactly 1 argument, got {}", arg_tys.len()),
                        Span::new(0, 0),
                    ).with_help("pass exactly 1 argument"));
                }
                if !matches!(arg_tys[0], Ty::List(_) | Ty::Dict(_) | Ty::Set(_)) {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `{}`",
                            arg_tys[0].name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass a `list[T]`, `dict[K, V]`, or `set[T]` value"));
                }
                return Ok(Ty::Int);
            }
            if let Some(symbol) = std_qualified_symbol(callee) {
                // Post-review finding: see `std_receiver_shadowed`'s own
                // doc comment -- a real local/parameter named `math`
                // shadows the stdlib module.
                let receiver = std_receiver_name(callee);
                if env.lookup(receiver).is_some() || is_local(local_names, receiver) {
                    return Err(std_receiver_shadowed(callee));
                }
                let pycc_std::StdSymbolKind::Function {
                    arg_tys: expected_arg_tys,
                    ret_ty,
                } = symbol.kind
                else {
                    return Err(std_constant_is_not_callable(callee));
                };
                if arg_tys.len() != expected_arg_tys.len() {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`{callee}` expects {} argument(s), got {}",
                            expected_arg_tys.len(),
                            arg_tys.len()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("pass exactly {} argument(s)", expected_arg_tys.len())));
                }
                for (arg_ty, expected) in arg_tys.iter().zip(expected_arg_tys) {
                    if *arg_ty != std_scalar_to_ty(*expected) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "`{callee}` expects `{}`, got `{}`",
                                std_scalar_to_ty(*expected).name(),
                                arg_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("pass a `{}` value", std_scalar_to_ty(*expected).name())));
                    }
                }
                return Ok(std_scalar_to_ty(ret_ty));
            }
            if callee == "float" && env.lookup_function(callee).is_none() {
                // D-086's own remedy for the int-to-float boundary: a hand-recognized
                // builtin, same as `len` above, not a user-declarable signature --
                // except, unlike `print`/`len`, a program can predate this builtin's
                // introduction with its own `def float(...)`. Reviewer finding
                // (post-merge review): unlike `print`/`len`, which have been
                // hand-recognized since before this compiler could compile
                // user-declared functions at all, `float` was undefined until
                // this issue, so a user-defined `float` was a valid, working
                // program on `main` immediately before this change landed --
                // silently reinterpreting it as this builtin would be a real
                // regression, not an inherited, already-accepted precedent.
                // `env.lookup_function` takes priority; only fall through to the
                // builtin when no such definition exists.
                // Always returns `Ty::Float` regardless of the argument's own type,
                // once that argument is numeric-like -- unlike `len`, there is no
                // homogeneity/element-type question to defer.
                if arg_tys.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!("`float` expects exactly 1 argument, got {}", arg_tys.len()),
                        Span::new(0, 0),
                    ).with_help("pass exactly 1 argument"));
                }
                if !matches!(arg_tys[0], Ty::Int | Ty::Float | Ty::Bool) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`float` expects an `int`, `float`, or `bool` argument, got `{}`",
                            arg_tys[0].name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass an `int`, `float`, or `bool` value"));
                }
                return Ok(Ty::Float);
            }
            // D-154 (Part 1 of #375): `ClassName(args)` (instantiation)
            // reuses this same generic `HirExpr::Call` node -- there is no
            // dedicated HIR shape for it (`pycc_hir::class`'s own doc
            // comment) -- so it is resolved here, checked before the
            // ordinary ("ClassName" as a plain function) lookup below, the
            // same precedence a generic-function call already gets just
            // above. `pycc_hir::lower_checked` enforces that a class name
            // can never collide with a top-level function, type-alias, or
            // import name in this compiler's flat, single-namespace model
            // -- both directions are rejected with `C0001` at HIR-lowering
            // time, before `env` is ever built (D-068 review finding on
            // #385; `crates/pycc_hir/src/lib.rs`'s `lower_checked`) -- so
            // trying the class table first here is unambiguous, not merely
            // an assumption.
            if env.lookup_class(callee).is_some() {
                return class::resolve_instantiation(env, callee, arg_tys);
            }
            // D-133/D-134: a call to a PEP 695 generic function is resolved
            // through call-site substitution, not through the ordinary
            // `functions` signature -- `env.lookup_function(callee)` would
            // otherwise see a signature still carrying `Ty::Param` and
            // reject every real (concrete) argument as an assignability
            // mismatch. Checked before the ordinary lookup below so this
            // takes precedence for every generic function, including one
            // reached recursively while inferring a nested call's own
            // arguments (e.g. `print(identity(1))`).
            if let Some(generic_func) = env.lookup_generic(callee) {
                return Ok(instantiate_generic_call(generic_func, arg_tys)?.return_ty);
            }
            let Some((param_tys, return_ty)) = env.lookup_function(callee) else {
                // Issue #142: before falling back to T0021 ("call to undefined
                // function"), check whether `callee` is a known Python 3.14
                // callable builtin that this compiler version does not implement
                // (e.g. `ValueError`, `Exception`, `int`, `range`). Such a call
                // is valid Python -- the builtin genuinely exists -- so it is a
                // capability gap (`C0001`), not a name-resolution failure
                // (`T0021`). This check is deliberately *after* the user-defined
                // function lookup, the `print`/`len`/`float` special cases, the
                // stdlib-qualified symbol lookup, the class-instantiation lookup,
                // and the generic-function lookup, so a user `def
                // ValueError(...)` always takes priority over this classification.
                if is_known_callable_builtin(callee) {
                    return Err(unsupported_callable_builtin(callee));
                }
                return Err(Diagnostic::error(
                    "T0021",
                    format!("call to undefined function `{callee}`"),
                    Span::new(0, 0),
                ));
            };
            // Issue #22: in top-level code, a call to a function whose
            // `def` has not been encountered yet in source order is a
            // static error -- CPython raises `NameError` at runtime for
            // the same case. Function bodies are exempt (all functions
            // are marked defined in `child_for_function`) because Python
            // evaluates a function body at call time, by which point all
            // module-level `def`s have typically executed.
            if !env.defined_functions.contains(callee.as_str()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot call function `{callee}` before its definition \
                         (NameError in CPython: name '{callee}' is not defined)"
                    ),
                    Span::new(0, 0),
                ));
            }
            if arg_tys.len() != param_tys.len() {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "`{callee}` expects {} argument(s), got {}",
                        param_tys.len(),
                        arg_tys.len()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("pass exactly {} argument(s)", param_tys.len())));
            }
            for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(param_tys.iter()).enumerate() {
                if !is_assignable(arg_ty.clone(), param_ty.clone()) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "argument {} of `{callee}` expects `{}`, got `{}`",
                            i + 1,
                            param_ty.name(),
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("pass a `{}` value", param_ty.name())));
                }
            }
            Ok(return_ty.clone())
        }
        HirExpr::ListLiteral(elements) => {
            // D-105: an empty list literal has no element to infer a type
            // from, and v0.2 has no `list[T]` annotation syntax to recover
            // it from instead -- reject plainly rather than letting
            // `Ty::Infer` leak into codegen. Reuses T0021 (an unconstrained
            // variable with no way to determine its type), not a new code
            // -- this is that same failure shape, not a distinct one.
            if elements.is_empty() {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty list literal's element type cannot be inferred without an annotation (list[T] annotations are not supported yet, D-105)".to_string(),
                    Span::new(0, 0),
                ));
            }
            let mut elem_ty: Option<Ty> = None;
            for element in elements {
                let this_ty = infer_expr_in(env, local_names, element)?;
                match &elem_ty {
                    None => elem_ty = Some(this_ty),
                    Some(expected) if *expected == this_ty => {}
                    Some(expected) => {
                        // Exact `Ty` equality, not `is_assignable`'s
                        // bool-is-an-int-subtype rule used elsewhere in this
                        // file -- D-105 requires every element to share the
                        // *exact same* `Ty`, so `[1, True]` is T0032 even
                        // though a bare `bool` is assignable to `int`.
                        return Err(Diagnostic::error(
                            "T0032",
                            format!(
                                "list element type mismatch: expected {} (from the first element), found {}",
                                expected.name(),
                                this_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("use a `{}` value here", expected.name())));
                    }
                }
            }
            let elem_ty = elem_ty.expect("checked non-empty above");
            // This is the one place a list literal's element type becomes
            // known with a real source construct behind it (D-105's
            // Consequences) -- deliberately placed here rather than as a
            // separate pre-codegen pass. Everything above this gate (the
            // homogeneity check) is fully generic over any scalar `Ty`; a
            // future PR widening codegen to e.g. `list[str]` only has to
            // relax this one check.
            if elem_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list[{}] is not compiled yet (D-105) -- only list[int] is",
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::List(Box::new(elem_ty)))
        }
        // PR-11 Task 3 (D-123): mirrors `ListLiteral`'s own homogeneity
        // check above, extended to a key/value pair, plus a `dict[str,
        // int]`-only gate mirroring `ListLiteral`'s own `T0034` gate
        // (D-122: "exactly one combination gets real codegen").
        HirExpr::DictLiteral(pairs) => {
            let Some((first_key, first_value)) = pairs.first() else {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty dict literal's key/value types cannot be inferred without an annotation (dict[K, V] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            };
            let key_ty = infer_expr_in(env, local_names, first_key)?;
            let val_ty = infer_expr_in(env, local_names, first_value)?;
            for (key, value) in &pairs[1..] {
                let this_key_ty = infer_expr_in(env, local_names, key)?;
                let this_val_ty = infer_expr_in(env, local_names, value)?;
                // Exact `Ty` equality on both key and value, not
                // `is_assignable`'s bool-is-an-int-subtype rule -- same
                // reasoning as `ListLiteral`'s own homogeneity check.
                if this_key_ty != key_ty || this_val_ty != val_ty {
                    return Err(Diagnostic::error(
                        "T0035",
                        format!(
                            "dict entry type mismatch: expected {}: {} (from the first pair), found {}: {}",
                            key_ty.name(),
                            val_ty.name(),
                            this_key_ty.name(),
                            this_val_ty.name(),
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("use a `{}` key and `{}` value here", key_ty.name(), val_ty.name())));
                }
            }
            let dict_ty = Ty::Dict(Box::new((key_ty, val_ty)));
            if dict_ty != Ty::Dict(Box::new((Ty::Str, Ty::Int))) {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "{} is not compiled yet (D-122) -- only dict[str, int] is",
                        dict_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(dict_ty)
        }
        // PR-11 Task 7 (D-123): mirrors `ListLiteral`'s own homogeneity
        // check above, for a single-element-type container (no key/value
        // pair), plus a `set[int]`-only gate mirroring `ListLiteral`'s own
        // `T0034` gate (D-122: "exactly one combination gets real codegen").
        // Unlike `DictLiteral` above, the empty-literal branch below is
        // unreachable from any real Python source: `{}` always parses as an
        // empty *dict* (Python has no empty-set literal spelling at all --
        // `set()` is a call, not a literal), so this only fires for a
        // hand-built `HirExpr::SetLiteral(vec![])` (e.g. this file's own
        // unit tests).
        HirExpr::SetLiteral(elements) => {
            let Some(first) = elements.first() else {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty set literal's element type cannot be inferred without an annotation (set[T] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            };
            let elem_ty = infer_expr_in(env, local_names, first)?;
            for element in &elements[1..] {
                let this_ty = infer_expr_in(env, local_names, element)?;
                // Exact `Ty` equality, not `is_assignable`'s
                // bool-is-an-int-subtype rule -- same reasoning as
                // `ListLiteral`'s own homogeneity check.
                if this_ty != elem_ty {
                    return Err(Diagnostic::error(
                        "T0037",
                        format!(
                            "set element type mismatch: expected {} (from the first element), found {}",
                            elem_ty.name(),
                            this_ty.name(),
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("use a `{}` value here", elem_ty.name())));
                }
            }
            let set_ty = Ty::Set(Box::new(elem_ty));
            if set_ty != Ty::Set(Box::new(Ty::Int)) {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "{} is not compiled yet (D-122) -- only set[int] is",
                        set_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(set_ty)
        }
        // PR-11b Task 3 (D-116): unlike `ListLiteral`/`DictLiteral`/
        // `SetLiteral`'s homogeneity checks, this arm allows *any* mix of
        // accepted element types -- heterogeneity is tuple's own defining
        // feature. The gate is per-element type membership (int/bool/float
        // only), not agreement with a first element's type.
        HirExpr::TupleLiteral(elements) => {
            if elements.is_empty() {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty tuple literal's element types cannot be inferred without an annotation (tuple[...] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            }
            let mut elem_tys = Vec::with_capacity(elements.len());
            for element in elements {
                let this_ty = infer_expr_in(env, local_names, element)?;
                if !matches!(this_ty, Ty::Int | Ty::Bool | Ty::Float) {
                    return Err(Diagnostic::error(
                        "T0039",
                        format!(
                            "tuple element type `{}` is not compiled yet (D-116) -- only int/bool/float elements are",
                            this_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                elem_tys.push(this_ty);
            }
            Ok(Ty::Tuple(Box::new(elem_tys)))
        }
        HirExpr::Subscript { base, index } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            let index_ty = infer_expr_in(env, local_names, index)?;
            match base_ty {
                Ty::List(elem_ty) => {
                    // Reuses T0021 (an unconstrained/conflicting-constraint
                    // shape), not a new code -- a non-int-compatible index is
                    // that same "operand type mismatch" failure, not a
                    // distinct one.
                    //
                    // Uses `is_assignable`, not exact `Ty` equality: D-086
                    // already established that `bool` is accepted wherever
                    // `int` is expected at an operand boundary (mirroring
                    // `is_assignable`'s own existing param/assignment rule),
                    // and indexing is exactly that kind of boundary --
                    // `xs[True]` is ordinary, CPython-valid Python (`bool` is
                    // an `int` subtype, PEP 285), not a type error.
                    // `pycc_codegen`'s `to_numeric_encoded_int` already has a
                    // `Scalar::Bool` arm reached unconditionally by every
                    // subscript index, so this was a pure over-rejection in
                    // the type checker, not a missing codegen capability.
                    if !is_assignable(index_ty.clone(), Ty::Int) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!("list index must be `int`, found `{}`", index_ty.name()),
                            Span::new(0, 0),
                        ).with_help("use an `int` value"));
                    }
                    Ok(*elem_ty)
                }
                // PR-11 Task 3 (D-123): `d[k]` read. Uses exact `Ty`
                // equality on the key, not `is_assignable` -- every
                // `Ty::Dict` value that survives `DictLiteral`'s own
                // `T0036` gate above has key type exactly `Ty::Str` (no
                // other combination reaches codegen, and no other source
                // construct can produce a `Ty::Dict` value at all), so
                // there is no bool/int-style widening question here, unlike
                // the list-index case above.
                Ty::Dict(kv) => {
                    let (key_ty, val_ty) = *kv;
                    if index_ty != key_ty {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "dict key type mismatch: expected `{}`, found `{}`",
                                key_ty.name(),
                                index_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("use a `{}` value here", key_ty.name())));
                    }
                    Ok(val_ty)
                }
                // PR-11b Task 3 (D-116): `t[k]` requires `k` to be a
                // literal, non-negative, in-bounds integer -- not merely
                // `int`-typed, unlike `List`'s case above. A heterogeneous
                // tuple's element type at position `k` is only knowable
                // when `k` is known at compile time, so every failure shape
                // (non-literal, negative, out-of-range) shares one code.
                Ty::Tuple(elems) => {
                    let HirExpr::IntLiteral(literal_index) = index.as_ref() else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ).with_help("use a literal, non-negative integer index within range"));
                    };
                    let Ok(literal_index) = usize::try_from(*literal_index) else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ).with_help("use a literal, non-negative integer index within range"));
                    };
                    let Some(elem_ty) = elems.get(literal_index) else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ).with_help("use a literal, non-negative integer index within range"));
                    };
                    Ok(elem_ty.clone())
                }
                // PR-11 Task 7 (D-123): `Ty::Set` deliberately has no
                // explicit arm here and falls through to this rejection --
                // real Python sets are not subscriptable either (`s[0]`
                // raises `TypeError` in CPython too), so this is not a v0.2
                // scope cut needing its own arm, it is the semantically
                // correct behavior for every other base type as well.
                other => Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support indexing", other.name()),
                    Span::new(0, 0),
                )),
            }
        }
        // PR-12 Task 7 (D-118): `base[start:stop:step]` read. Only
        // `list[int]` ships slicing in v0.2 -- every other base (including
        // `Ty::Dict`/`Ty::Set`, which real CPython also rejects for `[i:j]`,
        // and `Ty::Tuple`, an explicit deferral rather than a "never
        // supported" case) reuses the same `T0033` code `Subscript`'s own
        // `other` fallthrough above already established for non-indexable
        // bases, not a new one. A `list[T]` with `T != Ty::Int` reuses
        // `ListLiteral`'s own `T0034` ("only list[int] is compiled") gate.
        // Diagnostic order is deliberately base-type (`T0033`) before
        // element-type (`T0034`) before any bound's own type (`T0021`),
        // mirroring this file's existing "callee/base-type errors before
        // argument errors" convention (D-110's callee-first precedent,
        // applied here to base-type-before-bound-type) -- pinned by
        // `slicing_reports_the_base_type_error_before_any_bound_error` and
        // `slicing_reports_the_element_type_error_before_any_bound_error`
        // below.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            let Ty::List(elem_ty) = &base_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!(
                        "`{}` does not support slicing (only list[int] does)",
                        base_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            };
            if **elem_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list codegen only supports `list[int]` in v0.2, cannot slice `list[{}]`",
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            // Each bound is independently optional (`xs[:]`, `xs[1:]`,
            // `xs[:3]`, `xs[::2]` all parse) and, when present, an
            // arbitrary runtime `int`-typed expression -- not a literal-only
            // check -- so `is_assignable` (not exact `Ty` equality) applies
            // here, same as `Subscript`'s own index check above: `xs[True:]`
            // is ordinary, CPython-valid Python (`bool` is an `int`
            // subtype, PEP 285), not a type error. `step`'s runtime
            // positivity is deliberately not validated here -- it can't be,
            // for a non-literal runtime expression, at compile time; that
            // check is a later task's job (D-118).
            for (label, bound) in [("start", start), ("stop", stop), ("step", step)] {
                if let Some(bound) = bound {
                    let bound_ty = infer_expr_in(env, local_names, bound)?;
                    if !is_assignable(bound_ty.clone(), Ty::Int) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!("slice {label} must be `int`, got `{}`", bound_ty.name()),
                            Span::new(0, 0),
                        ).with_help("use an `int` value"));
                    }
                }
            }
            Ok(base_ty.clone())
        }
        HirExpr::ListAppend { list, value } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.append()`", list_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let value_ty = infer_expr_in(env, local_names, value)?;
            // Uses `is_assignable`, not exact `Ty` equality -- matching the
            // `Subscript` index check above (D-086): `x = [1]; x.append(True)`
            // is ordinary, CPython-valid Python (`bool` is an `int` subtype),
            // and `pycc_codegen`'s `MirExpr::ListAppend` arm already routes
            // the appended value through `to_encoded_int`, preserving a
            // `Scalar::Bool` marker while validating the int-compatible
            // payload unconditionally, so there is no missing codegen
            // capability here either. This is NOT the same question as `ListLiteral`'s
            // own homogeneity check above (`[1, True]`'s element type is
            // genuinely ambiguous to infer -- there is no already-known
            // `elem_ty` to check against); `.append()` on an *already-typed*
            // `list[int]` has no such ambiguity, so the looser
            // `is_assignable` rule applies here, not there. Reuses T0021 (a
            // call-site/assignment constraint mismatch), not a new code.
            if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot append `{}` to a list of `{}`",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", elem_ty.name(), value_ty.name())));
            }
            Ok(Ty::None)
        }
        // PR-12 Task 10 (D-119): `list.pop()`. Unlike `ListAppend`, this
        // arm's result is the list's own element type, not `Ty::None` --
        // `.pop()` is meant to be used for its value.
        HirExpr::ListPop { list } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.pop()`", list_ty.name()),
                    Span::new(0, 0),
                ));
            };
            Ok((**elem_ty).clone())
        }
        // PR-12 Task 10 (D-119): `dict.get(key, default)`. The key check
        // uses exact `Ty` equality, not `is_assignable` -- same reasoning as
        // `Subscript`'s own `Ty::Dict` read arm and `check_dict_set` above
        // (every `Ty::Dict` value that survives `DictLiteral`'s own `T0036`
        // gate has key type exactly `Ty::Str`, so there is no bool/int-style
        // widening question for a dict *key*, unlike a list index or a
        // dict *value*). The `default` check does use `is_assignable`,
        // mirroring `ListAppend`'s/`check_dict_set`'s own value-position
        // leniency (D-086 `bool`-subtypes-`int`). The overall result is the
        // dict's *value* type, never `Ty::None`, since a missing key still
        // yields the (same-typed) default rather than `None`.
        HirExpr::DictGetOrDefault { dict, key, default } => {
            let dict_ty = lookup_bound_name(env, local_names, dict)?;
            let Ty::Dict(kv) = &dict_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.get()`", dict_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let key_ty = infer_expr_in(env, local_names, key)?;
            if key_ty != kv.0 {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot look up a `{}` key in a dict of `{}` keys",
                        key_ty.name(),
                        kv.0.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", kv.0.name(), key_ty.name())));
            }
            let default_ty = infer_expr_in(env, local_names, default)?;
            if !is_assignable(default_ty.clone(), kv.1.clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot use a `{}` default for a dict of `{}` values",
                        default_ty.name(),
                        kv.1.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", kv.1.name(), default_ty.name())));
            }
            Ok(kv.1.clone())
        }
        // PR-12 Task 10 (D-119): `set.add(value)`. Mirrors `ListAppend`
        // exactly -- always `Ty::None`, with D-131's ordinary assignment
        // storage available when that result is bound to a name.
        HirExpr::SetAdd { set, value } => {
            let set_ty = lookup_bound_name(env, local_names, set)?;
            let Ty::Set(elem_ty) = &set_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.add()`", set_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let value_ty = infer_expr_in(env, local_names, value)?;
            if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot add `{}` to a set of `{}`",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", elem_ty.name(), value_ty.name())));
            }
            Ok(Ty::None)
        }
        // D-154 (Part 1 of #375): an instance attribute read/method call --
        // see `class::resolve_attr_get`/`class::resolve_method_call` for the
        // actual resolution (both shared with `check_stmt`'s own
        // `HirStmt::AttrSet` arm, which needs the identical attribute-type
        // lookup for its own assigned-value check).
        HirExpr::AttrGet { base, attr } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            class::resolve_attr_get(env, &base_ty, attr)
        }
        HirExpr::MethodCall { base, method, args } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            let arg_tys = args
                .iter()
                .map(|arg| infer_expr_in(env, local_names, arg))
                .collect::<Result<Vec<_>, _>>()?;
            class::resolve_method_call(env, &base_ty, method, &arg_tys)
        }
    }
}

fn is_assignable(from: Ty, to: Ty) -> bool {
    from == to || (from == Ty::Bool && to == Ty::Int) // bool is a subtype of int, TYPE_SYSTEM.md's representation table
}

fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
    if left == Ty::Str && right == Ty::Str {
        return if op == BinOpKind::Add {
            Ok(Ty::Str)
        } else {
            Err(Diagnostic::error(
                "T0021",
                format!("operator {op:?} is not defined for `str` and `str`"),
                Span::new(0, 0),
            ))
        };
    }
    let as_numeric = |t: &Ty| match t {
        Ty::Bool | Ty::Int => Some(Ty::Int),
        Ty::Float => Some(Ty::Float),
        _ => None,
    };
    match (as_numeric(&left), as_numeric(&right)) {
        (Some(_), Some(_)) if op == BinOpKind::Div => Ok(Ty::Float),
        (Some(Ty::Int), Some(Ty::Int)) => Ok(Ty::Int),
        (Some(_), Some(_)) => Ok(Ty::Float),
        _ => Err(Diagnostic::error(
            "T0021",
            format!(
                "operator {op:?} is not defined for `{}` and `{}`",
                left.name(),
                right.name()
            ),
            Span::new(0, 0),
        )),
    }
}

fn numeric_or_bool_compatible(a: Ty, b: Ty) -> bool {
    let is_numeric_like = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool);
    (is_numeric_like(&a) && is_numeric_like(&b)) || (a == Ty::Str && b == Ty::Str)
}

fn check_range_operand(
    env: &Environment,
    position: &str,
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    check_range_operand_in(env, &[], position, expr)
}

fn check_range_operand_in(
    env: &Environment,
    local_names: &[&str],
    position: &str,
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    let actual = infer_expr_in(env, local_names, expr)?;
    if is_assignable(actual.clone(), Ty::Int) {
        Ok(())
    } else {
        Err(Diagnostic::error(
            "T0021",
            format!("range {position} expects `int`, got `{}`", actual.name()),
            Span::new(0, 0),
        ).with_help("pass an `int` value"))
    }
}

fn check_assignment(env: &mut Environment, target: &str, ty: Ty) -> Result<(), Diagnostic> {
    // Every value assignment re-shadows a same-named `def` (D-110),
    // including a compatible-type reassignment of a name that already has a
    // representation record -- that branch below returns without calling
    // `bind()`, so clearing only inside `bind()` left the def-rebound flag
    // permanently stuck for any name with a pre-`def` binding (PR #252's
    // round-5 review caught `helper = 1; def helper() -> int: ...;
    // helper = 2; helper()` resolving the function where CPython raises
    // `TypeError`). Cleared here unconditionally, matching the solver's own
    // unconditional clearing in its Assign arm. Function bodies operate on
    // `child_for_function` clones, so a body-local assignment clears only
    // that body's view, never the module-level fact.
    env.def_rebound.remove(target);
    // Issue #118 Part 1: use `lookup_any` (not `lookup`) so a maybe-bound name
    // being reassigned on the current path becomes definite, while the
    // first-assignment-wins representation (type) from the maybe-binding is
    // retained -- `lookup` would return `None` for a `Maybe` binding, wrongly
    // treating the reassignment as a fresh first binding.
    if let Some(previous) = env.lookup_any(target) {
        if !is_assignable(ty.clone(), previous.clone()) {
            return Err(Diagnostic::error(
                "T0023",
                format!(
                    "cannot assign `{}` to `{target}`, previously inferred as `{}`",
                    ty.name(),
                    previous.name()
                ),
                Span::new(0, 0),
            ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", previous.name(), ty.name())));
        }
        // Issue #118 Part 1: a compatible reassignment on the current path
        // upgrades a `Maybe` binding to `Definitely` (the name is now
        // definitely assigned on this path). The first-assignment-wins
        // representation (type) is retained -- `bind` with the *existing*
        // type, not `ty`, would be wrong here because `bind` takes the
        // passed type; instead, directly insert `Definitely(previous)` to
        // keep the sticky representation while upgrading the binding state.
        if matches!(env.binding_state(target), Some(BindingState::Maybe(_))) {
            env.bind(target.to_string(), previous);
        }
        return Ok(());
    }
    // issue #245: a value-less `AnnAssign` (`x: int`) records a declared
    // type without binding one. The first real assignment/valued
    // redeclaration reaching this point is validated against that
    // declaration, and -- on success -- the *declared* type becomes the
    // sticky representation, not `ty` itself, exactly as an
    // `AnnAssign{value: Some}` already makes its own annotation (not the
    // initializer's inferred type) the sticky representation just above in
    // `check_stmt`/`check_stmt_in_function`. Worked examples: `x: int; x =
    // 1` binds `Int` (matches); `x: int; x: bool = True` binds `Int` (the
    // *earlier* declaration wins, `bool` is merely assignable to it); `x:
    // int; x = "hello"` rejects with `T0026`, distinct from `T0023`
    // (nothing was "previously inferred" here -- it was declared, never
    // assigned).
    if let Some(declared) = env.declared_ty(target) {
        if !is_assignable(ty.clone(), declared.clone()) {
            return Err(Diagnostic::error(
                "T0026",
                format!(
                    "cannot assign `{}` to `{target}`, previously declared as `{target}: {}`",
                    ty.name(),
                    declared.name()
                ),
                Span::new(0, 0),
            ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", declared.name(), ty.name())));
        }
        env.declared.remove(target);
        env.bind(target.to_string(), declared);
        return Ok(());
    }
    env.bind(target.to_string(), ty);
    Ok(())
}

/// Resolves a comprehension's iterable (`pycc_hir::CompIter`) to the loop
/// variable's type, without binding it -- mirrors `HirStmt::ForList`'s own
/// resolution exactly (`check_stmt`/`check_stmt_in_function`'s existing
/// `ForList` arms), reused rather than duplicated a third time (PR-12,
/// D-117). Range/list/dict/set element-type resolution is identical to
/// `ForList`'s; a comprehension adds nothing new here.
fn resolve_comp_iter(
    env: &Environment,
    local_names: &[&str],
    iter: &CompIter,
) -> Result<Ty, Diagnostic> {
    match iter {
        CompIter::Range { start, stop, step } => {
            check_range_operand_in(env, local_names, "start", start)?;
            check_range_operand_in(env, local_names, "stop", stop)?;
            check_range_operand_in(env, local_names, "step", step)?;
            Ok(Ty::Int)
        }
        CompIter::Name(name) => {
            let base_ty = lookup_bound_name(env, local_names, name)?;
            match base_ty {
                Ty::List(elem_ty) => Ok(*elem_ty),
                Ty::Dict(kv) => Ok(kv.0),
                Ty::Set(elem_ty) => Ok(*elem_ty),
                other => Err(Diagnostic::error(
                    "T0033",
                    format!(
                        "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                        other.name()
                    ),
                    Span::new(0, 0),
                )),
            }
        }
    }
}

/// Issue #118 Part 1: joins two branch environments into `env` after an `if`
/// statement. The join lattice is: `Definitely` > `Maybe` > unbound.
///
/// - A name bound `Definitely` in **both** branches (with compatible types)
///   joins to `Definitely`.
/// - A name bound in only one branch (or `Maybe` in either) joins to `Maybe`.
/// - A name bound `Definitely` in one branch and `Maybe` in the other joins to
///   `Maybe`.
/// - A name unbound in one branch and bound in the other joins to `Maybe`.
/// - A name unbound in both branches stays unbound.
///
/// Types from both branches must be compatible (via `is_assignable`); a
/// mismatch produces `T0023`. The first-established representation (type)
/// wins, matching `check_assignment`'s first-assignment-wins rule.
fn join_if_branches(
    env: &mut Environment,
    body_env: &Environment,
    orelse_env: &Environment,
) -> Result<(), Diagnostic> {
    let mut joined: HashMap<String, BindingState> = HashMap::new();
    // Pass 1: process every name bound in the body branch. For each, look
    // up the orelse branch's state (if any) and join. This pass covers all
    // names in `body_env`; the `(None, None)` case never arises because we
    // iterate `body_env.bindings` directly (each entry is `Some` on the body
    // side by construction).
    for (name, body_state) in &body_env.bindings {
        let orelse_state = orelse_env.bindings.get(name);
        match (body_state, orelse_state) {
            // Both branches bind the name.
            (BindingState::Definitely(bt), Some(BindingState::Definitely(ot))) => {
                if bt != ot && !is_assignable(bt.clone(), ot.clone()) {
                    return Err(Diagnostic::error(
                        "T0023",
                        format!(
                            "cannot assign `{}` to `{name}`, previously inferred as `{}`",
                            ot.name(),
                            bt.name()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", bt.name(), ot.name())));
                }
                // First-assignment-wins: keep the body's type (matching
                // check_assignment's representation stickiness).
                joined.insert(name.clone(), BindingState::Definitely(bt.clone()));
            }
            // One or both branches have Maybe, or only one branch binds it.
            (BindingState::Definitely(ty), Some(BindingState::Maybe(_)))
            | (BindingState::Maybe(_), Some(BindingState::Definitely(ty)))
            | (BindingState::Maybe(ty), Some(BindingState::Maybe(_))) => {
                // Join of Definitely and Maybe = Maybe; Maybe and Maybe = Maybe.
                // Keep the type from whichever is available (first wins).
                joined.insert(name.clone(), BindingState::Maybe(ty.clone()));
            }
            // Body binds it, orelse does not -> Maybe.
            (BindingState::Definitely(ty), None)
            | (BindingState::Maybe(ty), None) => {
                joined.insert(name.clone(), BindingState::Maybe(ty.clone()));
            }
        }
    }
    // Pass 2: process names bound only in the orelse branch (not in body).
    // Each such name is Maybe (the body branch might have been the taken one).
    for (name, orelse_state) in &orelse_env.bindings {
        if body_env.bindings.contains_key(name) {
            continue; // already handled in pass 1
        }
        match orelse_state {
            BindingState::Definitely(ty) | BindingState::Maybe(ty) => {
                joined.insert(name.clone(), BindingState::Maybe(ty.clone()));
            }
        }
    }
    env.bindings = joined;
    Ok(())
}

/// Issue #118 Part 1: joins a loop body environment back into `env` after a
/// `while` or `for` loop. The loop body may execute zero times, so every
/// body-only binding joins back as `Maybe`. A name that was `Definitely`
/// bound before the loop stays `Definitely` (it was bound regardless of
/// whether the loop ran). A name that was `Maybe` before the loop and is also
/// bound in the body stays `Maybe`.
fn join_loop_body(env: &mut Environment, body_env: &Environment) {
    // For each name bound in the body but not already Definitely bound in env,
    // downgrade to Maybe. Names already Definitely bound in env are unchanged.
    for (name, state) in &body_env.bindings {
        match env.bindings.get(name) {
            Some(BindingState::Definitely(_)) => {
                // Already definite before the loop -- stays definite. But if
                // the body assigned a different (incompatible) type,
                // check_assignment already caught that inside the body check.
                // Keep the existing definite binding.
            }
            _ => {
                // Not bound in env, or maybe-bound: the body may or may not
                // have run, so this is Maybe.
                let ty = match state {
                    BindingState::Definitely(ty) | BindingState::Maybe(ty) => ty.clone(),
                };
                env.bindings.insert(name.clone(), BindingState::Maybe(ty));
            }
        }
    }
}

pub fn check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Assign { target, value } => {
            let ty = infer_expr(env, value)?;
            check_assignment(env, target, ty)
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
        } => {
            if let Some(value) = value {
                let inferred = infer_expr(env, value)?;
                if !is_assignable(inferred.clone(), annotation.clone()) {
                    return Err(Diagnostic::error(
                        "T0025",
                        format!(
                            "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                            inferred.name(),
                            annotation.name()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", annotation.name(), inferred.name())));
                }
                // Route through `check_assignment` (not a raw `env.bind`) so a
                // name's first-established representation stays sticky across
                // an annotated re-declaration, exactly as it already does for
                // plain `Assign` -- `pycc_mir`'s own `bind_variable` (D-040's
                // "first assignment fixes a binding's representation"
                // invariant) keeps the *first* recorded MIR type regardless of
                // a later compatible reassignment, so the checker's `env` must
                // agree or a later annotated reassignment (e.g. `x = 1` then
                // `x: str = "s"`, where `is_assignable(Str, Str)` alone would
                // wrongly accept it) could diverge from what codegen actually
                // stores.
                check_assignment(env, target, annotation.clone())?;
            } else {
                // No initializer: register no *binding* (a premature read
                // still raises the existing T0021 -- collect_local_names
                // (Step 1) already marked `target` local, and `declare`
                // never touches `bindings`), but do retain the declared
                // type (issue #245) so a later plain or annotated
                // assignment is checked against it instead of silently
                // treating the first later assignment as the initial,
                // unconstrained binding.
                env.declare(target.clone(), annotation.clone())?;
            }
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr(env, expr).map(|_| ()),
        HirStmt::If { test, body, orelse } => {
            infer_expr(env, test)?; // any type is accepted as truthy for v0.1 -- Python's own truthiness has no static type restriction
            // Issue #118 Part 1: check each branch in an independent clone of
            // env, then join the results. A no-else `if` makes all body-only
            // bindings `Maybe` (the orelse clone is empty, so every body
            // binding is "one branch only" -> Maybe).
            // Fast path: if neither branch introduces any new bindings, skip
            // the clone+join and check both branches in-place (matching the
            // pre-#118 behavior for guard-only ifs).
            if !introduces_bindings(body) && !introduces_bindings(orelse) {
                check_if_branches_in_place(env, body, orelse)
            } else {
                let mut body_env = env.clone();
                for stmt in body {
                    check_stmt(&mut body_env, stmt)?;
                }
                let mut orelse_env = env.clone();
                for stmt in orelse {
                    check_stmt(&mut orelse_env, stmt)?;
                }
                join_if_branches(env, &body_env, &orelse_env)
            }
        }
        HirStmt::While { test, body } => {
            infer_expr(env, test)?;
            // Issue #118 Part 1: the loop body may execute zero times, so
            // every body-only binding joins back as `Maybe`.
            // Fast path: if the body introduces no bindings, check in-place.
            if !introduces_bindings(body) {
                check_while_body_in_place(env, body)
            } else {
                let mut body_env = env.clone();
                for stmt in body {
                    check_stmt(&mut body_env, stmt)?;
                }
                join_loop_body(env, &body_env);
                Ok(())
            }
        }
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            check_range_operand(env, "start", start)?;
            check_range_operand(env, "stop", stop)?;
            check_range_operand(env, "step", step)?;
            // Issue #118 Part 1: track whether the loop variable was already
            // definitely bound before the loop. If so, it stays definite
            // after the loop (the variable was bound regardless of whether
            // the loop ran). If not, it is `Maybe` after the loop (the loop
            // may execute zero times).
            let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
            check_assignment(env, var, Ty::Int)?;
            let mut body_env = env.clone();
            for stmt in body {
                check_stmt(&mut body_env, stmt)?;
            }
            join_loop_body(env, &body_env);
            // Issue #118 Part 1: if the loop variable was not definitely bound
            // before the loop, downgrade it to Maybe (the loop may execute
            // zero times). A pre-bound variable stays Definitely bound.
            if !was_definite
                && let Some(ty) = env.lookup_any(var)
            {
                env.bind_maybe(var.to_string(), ty);
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            // Module (top-level) scope has no "local before assignment"
            // concept the way a function body does -- every other arm here
            // (e.g. `ExprStmt` via `infer_expr`) resolves names with an
            // empty `local_names` slice too, so an unresolved `list` is
            // simply "not defined," never `unbound_local`.
            let list_ty = lookup_bound_name(env, &[], list)?;
            // PR-11 Task 3 (D-123): `for k in d:` iterates a dict's own keys
            // in insertion order, mirroring `len()`'s own relaxation to
            // accept `Ty::Dict` alongside `Ty::List` at this crate's other
            // hand-recognized dispatch points. `HirStmt::ForList` itself is
            // reused unconditionally for any bare-name iterable, dict or
            // list alike (`pycc_hir`'s own lowering has no type information
            // to pick a different node) -- this is the point where the real
            // type is resolved. PR-11 Task 7 (D-123): `for x in s:` iterates
            // a set's own elements (order is this implementation's own
            // insertion order, not a CPython-matching guarantee -- see
            // D-123's own iteration-order caveat), so `Ty::Set` is accepted
            // here too, binding the loop variable as the set's element type.
            let var_ty = match list_ty {
                Ty::List(elem_ty) => *elem_ty,
                Ty::Dict(kv) => kv.0,
                Ty::Set(elem_ty) => *elem_ty,
                other => {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                            other.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            };
            // Issue #118 Part 1: track whether the loop variable was already
            // definitely bound before the loop (see ForRange above).
            let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
            check_assignment(env, var, var_ty)?;
            let mut body_env = env.clone();
            for stmt in body {
                check_stmt(&mut body_env, stmt)?;
            }
            join_loop_body(env, &body_env);
            // Issue #118 Part 1: if the loop variable was not definitely bound
            // before the loop, downgrade it to Maybe (the loop may execute
            // zero times). A pre-bound variable stays Definitely bound.
            if !was_definite
                && let Some(ty) = env.lookup_any(var)
            {
                env.bind_maybe(var.to_string(), ty);
            }
            Ok(())
        }
        // PR-12 Task 3 (D-117): `target = [elt for var in iter [if cond]]`
        // at module scope. `var` is resolved and bound exactly like
        // `ForList`'s own loop variable above (via the shared
        // `resolve_comp_iter` helper) *before* `cond`/`elt` are checked, so
        // a reference to the loop variable inside either sub-expression
        // resolves correctly. The produced element type is gated to
        // `Ty::Int` -- identical rule to `ListLiteral`'s own `T0034` gate
        // (D-105/D-122), just reached via a new code path (D-119); no new
        // diagnostic code is minted.
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, &[], iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr(env, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let elt_ty = infer_expr(env, elt)?;
            if elt_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list codegen only supports `list[int]` in v0.2, got a comprehension producing `list[{}]`",
                        elt_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            check_assignment(env, target, Ty::List(Box::new(Ty::Int)))
        }
        // PR-12 Task 3 (D-117): `target = {elt for var in iter [if cond]}`
        // at module scope. Mirrors `ListCompAssign` above exactly except for
        // the produced type and diagnostic code (D-119: reuses `T0038`,
        // identical to `SetLiteral`'s own gate).
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, &[], iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr(env, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let elt_ty = infer_expr(env, elt)?;
            if elt_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "set codegen only supports `set[int]` in v0.2, got a comprehension producing `set[{}]`",
                        elt_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            check_assignment(env, target, Ty::Set(Box::new(Ty::Int)))
        }
        // PR-12 Task 3 (D-117): `target = {key: value for var in iter [if
        // cond]}` at module scope. Mirrors `ListCompAssign` above except for
        // the key/value split (D-119: reuses `T0036`, identical to
        // `DictLiteral`'s own gate).
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let var_ty = resolve_comp_iter(env, &[], iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr(env, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let key_ty = infer_expr(env, key)?;
            let value_ty = infer_expr(env, value)?;
            if key_ty != Ty::Str || value_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "dict codegen only supports `dict[str, int]` in v0.2, got a comprehension producing `dict[{}, {}]`",
                        key_ty.name(),
                        value_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            check_assignment(env, target, Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        }
        HirStmt::Return(_) => Err(Diagnostic::error(
            "T0024",
            "'return' outside a function is not allowed".to_string(),
            Span::new(0, 0),
        )),
        HirStmt::DictSet { dict, key, value } => check_dict_set(env, &[], dict, key, value),
        HirStmt::AttrSet { base, attr, value } => {
            class::check_attr_set(env, &[], base, attr, value)
        }
    }
}

/// `d[k] = v` (PR-11 Task 3, D-123): insert-or-update. Shared between
/// module (`check_stmt`, `local_names = &[]`) and function-body
/// (`check_stmt_in_function`) scope, mirroring how `check_range_operand`/
/// `check_range_operand_in` are already split for `ForRange`. Reuses
/// `T0033` for a non-`dict` base (matching `ListAppend`'s own "does not
/// support X" shape) and `T0021` for a key or value type mismatch
/// (matching `Subscript`'s and `ListAppend`'s own reuse of `T0021` for an
/// operand/assignment constraint mismatch).
fn check_dict_set(
    env: &Environment,
    local_names: &[&str],
    dict: &str,
    key: &HirExpr,
    value: &HirExpr,
) -> Result<(), Diagnostic> {
    let dict_ty = lookup_bound_name(env, local_names, dict)?;
    let Ty::Dict(kv) = &dict_ty else {
        return Err(Diagnostic::error(
            "T0033",
            format!("`{}` does not support item assignment", dict_ty.name()),
            Span::new(0, 0),
        ));
    };
    let (key_ty, val_ty) = kv.as_ref();
    // Exact `Ty` equality on the key, not `is_assignable` -- same reasoning
    // as `Subscript`'s own `Ty::Dict` read arm.
    let key_expr_ty = infer_expr_in(env, local_names, key)?;
    if key_expr_ty != *key_ty {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "dict key type mismatch: expected `{}`, found `{}`",
                key_ty.name(),
                key_expr_ty.name()
            ),
            Span::new(0, 0),
        ).with_help(format!("use a `{}` value here", key_ty.name())));
    }
    let value_ty = infer_expr_in(env, local_names, value)?;
    if !is_assignable(value_ty.clone(), val_ty.clone()) {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "cannot assign `{}` to a dict value of `{}`",
                value_ty.name(),
                val_ty.name()
            ),
            Span::new(0, 0),
        ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", val_ty.name(), value_ty.name())));
    }
    Ok(())
}

/// Checks `function` in complete isolation from any module -- no sibling
/// function signatures, no module-level globals, and (D-154, Part 1 of
/// #375) no class table either, since this entry point takes a bare
/// `&HirItem` with no enclosing `&HirModule` to source `class_defs` from.
/// A body that calls a sibling function or reads a module global already
/// couldn't check here before D-154 existed; a body that instantiates a
/// class, or reads/writes/calls a method on an instance, is the same kind
/// of gap, not a new one -- `Environment::classes` is simply empty.
/// Production compilation never reaches this function: `check`/
/// `check_and_resolve` always build their `Environment` from a real
/// `HirModule` (via `class::bind_classes`), so this isolation only affects
/// a caller that deliberately checks one function outside any module
/// context -- this crate's own unit tests, and the workspace's own
/// direct-API integration tests (e.g. `tests/slice0.rs`), today.
pub fn check_function(function: &HirItem) -> Result<(), Diagnostic> {
    let local_names = match function {
        HirItem::Function { params, body, .. } => function_local_names(params, body),
        HirItem::TopLevelStmt(_) => Vec::new(),
    };
    check_function_in(&Environment::new(), function, &local_names)
}

/// Checks one function's body, resolving sibling calls and module-level
/// global reads against a clone of `module_env` (see D-040/D-041/D-055) instead
/// of an isolated, self-only scope. Lexically local binding targets are removed
/// from that clone before the body is checked. The clone owns independent value
/// bindings while sharing the immutable function registry through copy-on-write
/// storage, so a function's parameters and local assignments never leak back
/// into the module scope or into any other function's check.
fn check_function_in(
    module_env: &Environment,
    function: &HirItem,
    local_names: &[&str],
) -> Result<(), Diagnostic> {
    let HirItem::Function {
        name,
        params,
        return_ty,
        body,
    } = function
    else {
        panic!("check_function called with a non-Function HirItem");
    };
    let standalone_params;
    let (resolved_params, resolved_return, signature_was_registered) =
        if let Some((param_tys, return_ty)) = module_env.lookup_function(name) {
            (param_tys.as_slice(), return_ty.clone(), true)
        } else {
            standalone_params = params.iter().map(|(_, ty)| ty.clone()).collect::<Vec<_>>();
            (standalone_params.as_slice(), return_ty.clone(), false)
        };
    if resolved_params.contains(&Ty::Infer) || resolved_return == Ty::Infer {
        return Err(Diagnostic::error(
            "T0021",
            format!("cannot check private helper `{name}` before its signature is inferred"),
            Span::new(0, 0),
        ));
    }
    let mut env = module_env.child_for_function(local_names);
    if !signature_was_registered {
        env.bind_function(
            name.clone(),
            resolved_params.to_vec(),
            resolved_return.clone(),
        );
    }
    for ((param_name, _), param_ty) in params.iter().zip(resolved_params.iter().cloned()) {
        env.bind(param_name.clone(), param_ty);
    }
    for stmt in body {
        check_stmt_in_function(&mut env, local_names, stmt, resolved_return.clone())?;
    }
    if resolved_return != Ty::None && !block_always_returns(body) {
        return Err(Diagnostic::error(
            "T0022",
            format!(
                "function `{name}` can exit without returning `{}`",
                resolved_return.name()
            ),
            Span::new(0, 0),
        ).with_help(format!("return a `{}` value", resolved_return.name())));
    }
    Ok(())
}

fn block_always_returns(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Return(_) => true,
        HirStmt::If { body, orelse, .. } => {
            !orelse.is_empty() && block_always_returns(body) && block_always_returns(orelse)
        }
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::While { .. }
        | HirStmt::ForRange { .. }
        | HirStmt::ForList { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        // PR-12 Task 3 (D-117): a comprehension statement never contains a
        // `return` (its `elt`/`cond`/`key`/`value` are expressions, not
        // statements), so it can never make a block always return, exactly
        // like `Assign`/`ForList`/`DictSet` above.
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. } => false,
    })
}

fn check_stmt_in_function(
    env: &mut Environment,
    local_names: &[&str],
    stmt: &HirStmt,
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Return(None) => {
            if return_ty != Ty::None {
                return Err(Diagnostic::error(
                    "T0022",
                    format!(
                        "expected a return value of type `{}`, got none",
                        return_ty.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("return a `{}` value", return_ty.name())));
            }
            Ok(())
        }
        HirStmt::Return(Some(expr)) => {
            let actual = infer_expr_in(env, local_names, expr)?;
            if !is_assignable(actual.clone(), return_ty.clone()) {
                return Err(Diagnostic::error(
                    "T0022",
                    format!(
                        "expected return type `{}`, got `{}`",
                        return_ty.name(),
                        actual.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("return a `{}` value", return_ty.name())));
            }
            Ok(())
        }
        HirStmt::If { test, body, orelse } => {
            infer_expr_in(env, local_names, test)?;
            // Issue #118 Part 1: check each branch in an independent clone of
            // env, then join the results. A no-else `if` makes all body-only
            // bindings `Maybe`.
            // Fast path: if neither branch introduces any new bindings, skip
            // the clone+join and check both branches in-place.
            if !introduces_bindings(body) && !introduces_bindings(orelse) {
                check_if_branches_in_place_in_function(env, local_names, body, orelse, return_ty.clone())
            } else {
                let mut body_env = env.clone();
                for s in body {
                    check_stmt_in_function(&mut body_env, local_names, s, return_ty.clone())?;
                }
                let mut orelse_env = env.clone();
                for s in orelse {
                    check_stmt_in_function(&mut orelse_env, local_names, s, return_ty.clone())?;
                }
                join_if_branches(env, &body_env, &orelse_env)
            }
        }
        HirStmt::While { test, body } => {
            infer_expr_in(env, local_names, test)?;
            // Issue #118 Part 1: the loop body may execute zero times, so
            // every body-only binding joins back as `Maybe`.
            // Fast path: if the body introduces no bindings, check in-place.
            if !introduces_bindings(body) {
                check_while_body_in_place_in_function(env, local_names, body, return_ty.clone())
            } else {
                let mut body_env = env.clone();
                for s in body {
                    check_stmt_in_function(&mut body_env, local_names, s, return_ty.clone())?;
                }
                join_loop_body(env, &body_env);
                Ok(())
            }
        }
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            check_range_operand_in(env, local_names, "start", start)?;
            check_range_operand_in(env, local_names, "stop", stop)?;
            check_range_operand_in(env, local_names, "step", step)?;
            // Issue #118 Part 1: track whether the loop variable was already
            // definitely bound before the loop (see check_stmt's ForRange).
            let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
            check_assignment(env, var, Ty::Int)?;
            let mut body_env = env.clone();
            for s in body {
                check_stmt_in_function(&mut body_env, local_names, s, return_ty.clone())?;
            }
            join_loop_body(env, &body_env);
            // Issue #118 Part 1: if the loop variable was not definitely bound
            // before the loop, downgrade it to Maybe (the loop may execute
            // zero times). A pre-bound variable stays Definitely bound.
            if !was_definite
                && let Some(ty) = env.lookup_any(var)
            {
                env.bind_maybe(var.to_string(), ty);
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            // See the module-scope `check_stmt` arm's own comment (PR-11
            // Task 3, D-123): `for k in d:` iterates a dict's keys. PR-11
            // Task 7 (D-123): `for x in s:` iterates a set's elements.
            let var_ty = match list_ty {
                Ty::List(elem_ty) => *elem_ty,
                Ty::Dict(kv) => kv.0,
                Ty::Set(elem_ty) => *elem_ty,
                other => {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                            other.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            };
            // Issue #118 Part 1: track whether the loop variable was already
            // definitely bound before the loop (see check_stmt's ForList).
            let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
            check_assignment(env, var, var_ty)?;
            let mut body_env = env.clone();
            for s in body {
                check_stmt_in_function(&mut body_env, local_names, s, return_ty.clone())?;
            }
            join_loop_body(env, &body_env);
            // Issue #118 Part 1: if the loop variable was not definitely bound
            // before the loop, downgrade it to Maybe (the loop may execute
            // zero times). A pre-bound variable stays Definitely bound.
            if !was_definite
                && let Some(ty) = env.lookup_any(var)
            {
                env.bind_maybe(var.to_string(), ty);
            }
            Ok(())
        }
        // PR-12 Task 3 (D-117): function-scope counterparts of the
        // module-scope `check_stmt` arms above, `local_names`-aware
        // (`resolve_comp_iter`/`infer_expr_in` in place of the module-scope
        // helpers/`&[]`) -- otherwise identical, mirroring exactly how
        // `ForList`'s own two arms (module vs. function scope) already
        // differ only in that respect.
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, local_names, iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr_in(env, local_names, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let elt_ty = infer_expr_in(env, local_names, elt)?;
            if elt_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list codegen only supports `list[int]` in v0.2, got a comprehension producing `list[{}]`",
                        elt_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            check_assignment(env, target, Ty::List(Box::new(Ty::Int)))
        }
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, local_names, iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr_in(env, local_names, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let elt_ty = infer_expr_in(env, local_names, elt)?;
            if elt_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "set codegen only supports `set[int]` in v0.2, got a comprehension producing `set[{}]`",
                        elt_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            check_assignment(env, target, Ty::Set(Box::new(Ty::Int)))
        }
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let var_ty = resolve_comp_iter(env, local_names, iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr_in(env, local_names, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let key_ty = infer_expr_in(env, local_names, key)?;
            let value_ty = infer_expr_in(env, local_names, value)?;
            if key_ty != Ty::Str || value_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "dict codegen only supports `dict[str, int]` in v0.2, got a comprehension producing `dict[{}, {}]`",
                        key_ty.name(),
                        value_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            check_assignment(env, target, Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        }
        HirStmt::Assign { target, value } => {
            let ty = infer_expr_in(env, local_names, value)?;
            check_assignment(env, target, ty)
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
        } => {
            if let Some(value) = value {
                let inferred = infer_expr_in(env, local_names, value)?;
                if !is_assignable(inferred.clone(), annotation.clone()) {
                    return Err(Diagnostic::error(
                        "T0025",
                        format!(
                            "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                            inferred.name(),
                            annotation.name()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", annotation.name(), inferred.name())));
                }
                // See the module-scope `check_stmt` arm's comment: route
                // through `check_assignment` so a name's first-established
                // representation stays sticky, matching `pycc_mir`'s own
                // `bind_variable` invariant.
                check_assignment(env, target, annotation.clone())?;
            } else {
                // See the module-scope `check_stmt` arm's comment (issue
                // #245): retain the declared type via `env.declare` without
                // binding it, so a premature read still raises T0021 and a
                // later assignment is checked against the declaration.
                env.declare(target.clone(), annotation.clone())?;
            }
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr_in(env, local_names, expr).map(|_| ()),
        HirStmt::DictSet { dict, key, value } => check_dict_set(env, local_names, dict, key, value),
        HirStmt::AttrSet { base, attr, value } => {
            class::check_attr_set(env, local_names, base, attr, value)
        }
    }
}

fn t0042(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error("T0042", message.into(), Span::new(0, 0))
}

/// Recursively scans one signature-position `Ty` for `Ty::Param` occurrences
/// (D-133/D-134), threading the type parameter name found so far through
/// every call. `is_top_level` distinguishes a parameter/return type's own
/// top-level position (where a bare `Ty::Param` is the one shape v0.2
/// instantiates) from any position nested inside a container (`list[T]`,
/// `dict[str, T]`, ...), which D-134 rejects outright regardless of whether
/// the type parameter is otherwise consistent -- container-of-type-parameter
/// is out of scope independently of this defense-in-depth pass, matching
/// D-105's own pre-existing "container element type is fixed, not generic"
/// restriction.
///
/// Defense in depth, not a reachable frontend path: `crates/pycc_hir/src/lib.rs`'s
/// `lower_function` already enforces at most one PEP 695 `TypeVar` per
/// function (Task 1) and `annotation_to_ty` never lowers a `Subscript`
/// annotation at all, so a real `def f[T](x: list[T])` or `def f[T, U](...)`
/// cannot reach `pycc_types` from parsed source today -- this only fires
/// for a hand-constructed `HirItem` (a future frontend regression, or a
/// unit test exercising this function directly, mirroring how this file's
/// other "defense in depth" checks are exercised elsewhere).
fn scan_signature_ty_for_param(
    ty: &Ty,
    is_top_level: bool,
    found: &mut Option<String>,
) -> Result<(), Diagnostic> {
    match ty {
        Ty::Param(name) => {
            if !is_top_level {
                return Err(t0042(format!(
                    "type parameter `{name}` used inside a container position is not supported yet -- v0.2 only instantiates a bare type-parameter position, matching D-105's own fixed-container-element-type restriction"
                )));
            }
            match found {
                Some(existing) if existing != name.as_ref() => {
                    return Err(t0042(format!(
                        "generic functions with more than one type parameter are not supported yet (found both `{existing}` and `{name}`)"
                    )));
                }
                _ => *found = Some(name.to_string()),
            }
            Ok(())
        }
        Ty::List(elem) | Ty::Set(elem) => scan_signature_ty_for_param(elem, false, found),
        Ty::Dict(kv) => {
            scan_signature_ty_for_param(&kv.0, false, found)?;
            scan_signature_ty_for_param(&kv.1, false, found)
        }
        Ty::Tuple(elems) => {
            for elem in elems.iter() {
                scan_signature_ty_for_param(elem, false, found)?;
            }
            Ok(())
        }
        // D-154: `Ty::Instance` can never carry a `Ty::Param` -- see
        // `ty_contains_param`'s own identical arm/reasoning above.
        Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::None | Ty::Infer | Ty::Instance(_) => {
            Ok(())
        }
    }
}

/// Finds the single PEP 695 type-parameter name used across a function's
/// parameter and return types, or `None` if the function isn't generic at
/// all. Returns `T0042` if it finds two distinct names or any
/// container-position occurrence (see `scan_signature_ty_for_param`'s own
/// doc comment for why this is defense-in-depth rather than a reachable
/// frontend path).
fn generic_type_param_name(
    params: &[(String, Ty)],
    return_ty: &Ty,
) -> Result<Option<String>, Diagnostic> {
    let mut found = None;
    for (_, ty) in params {
        scan_signature_ty_for_param(ty, true, &mut found)?;
    }
    scan_signature_ty_for_param(return_ty, true, &mut found)?;
    Ok(found)
}

/// D-133/D-134: type-checks a generic function's body exactly once,
/// symbolically. `Ty::Param(name)` participates in the existing
/// `collect_expr_constraints`/`infer_expr_in` traversal (via `check_function`
/// below) as an ordinary, opaque `Ty` value: `merge_inferred_types`,
/// `is_assignable`, and `numeric_result_type` already compare `Ty` by
/// structural equality (plus the one `Bool`/`Int` special case), so
/// `Ty::Param("T") == Ty::Param("T")` unifies exactly like any other
/// self-consistent type, while `Ty::Param("T")` used where an `int`-only
/// operator expects a numeric operand (e.g. `x + 1` where `x: T`) already
/// falls through those functions' existing "no match" arms and produces the
/// same `T0021` a real type mismatch would -- no changes to either function
/// were needed for this. This function's own job is the shape gate
/// `check_function` cannot express on its own: reject more than one
/// distinct type-parameter name or any container-position occurrence
/// (`T0042`, defense in depth per `generic_type_param_name`'s doc comment)
/// before the body is checked at all.
///
/// Same module-isolation caveat as `check_function` (D-154, Part 1 of
/// #375): this entry point's own `Environment` has no class table either,
/// for the identical reason -- no `&HirModule` is available here to source
/// `class_defs` from.
pub fn check_generic_function(func: &HirItem) -> Result<(), Diagnostic> {
    let local_names = match func {
        HirItem::Function { params, body, .. } => function_local_names(params, body),
        HirItem::TopLevelStmt(_) => Vec::new(),
    };
    check_generic_function_in(&Environment::new(), func, &local_names)
}

/// Module-environment-aware counterpart to [`check_generic_function`],
/// mirroring `check_function`/`check_function_in`'s own existing split.
///
/// PR-13 final review (I3): a generic function's body is checked against the
/// module's function-signature environment exactly like an ordinary
/// function's body is, so a call to a *non-generic* sibling function
/// resolves normally instead of producing a factually false "call to
/// undefined function" `T0021`. The earlier env-less scoping was not a
/// design requirement -- a generic function's own type parameter is
/// resolved by call-site substitution, which is orthogonal to whether its
/// body can see its siblings.
///
/// PR-13 final review (Critical): before the body is checked at all, every
/// call in it is scanned and rejected with `T0042` when its callee is this
/// function itself or any other generic function registered in
/// `module_env`. `monomorphize` never rewrites calls that appear inside a
/// generic body (it drops the original generic item wholesale and emits
/// only substituted specializations), so such a call would survive into the
/// emitted specialization as a reference to a function that no longer
/// exists, and `pycc_mir` would panic on it. D-134's thin slice is
/// single-call-site monomorphization, not general recursive generic
/// instantiation, so this shape is rejected pre-codegen with a clear
/// diagnostic instead of being accepted by `check` and crashing `build`.
fn check_generic_function_in(
    module_env: &Environment,
    func: &HirItem,
    local_names: &[&str],
) -> Result<(), Diagnostic> {
    if let HirItem::Function {
        name,
        params,
        return_ty,
        body,
    } = func
    {
        generic_type_param_name(params, return_ty)?;
        reject_generic_calls_in_block(module_env, name, body)?;
    }
    check_function_in(module_env, func, local_names)
}

/// `T0042` for one rejected call inside a generic function's own body
/// (see `check_generic_function_in`).
fn reject_generic_call(own_name: &str, callee: &str) -> Diagnostic {
    if own_name == callee {
        t0042(format!(
            "generic function `{own_name}` calls itself -- a generic function cannot call itself or another generic function (recursive generic instantiation is not supported yet)"
        ))
    } else {
        t0042(format!(
            "generic function `{own_name}` calls generic function `{callee}` -- a generic function cannot call itself or another generic function (recursive generic instantiation is not supported yet)"
        ))
    }
}

/// Walks every statement in a generic function's body, rejecting any call
/// whose callee is `own_name` or a generic function registered in
/// `module_env`. Structurally mirrors `rewrite_generic_calls_in_stmt` --
/// every statement position that can hold an expression is visited.
fn reject_generic_calls_in_block(
    module_env: &Environment,
    own_name: &str,
    body: &[HirStmt],
) -> Result<(), Diagnostic> {
    for stmt in body {
        reject_generic_calls_in_stmt(module_env, own_name, stmt)?;
    }
    Ok(())
}

/// Pushes every expression position a comprehension's iterable can hold
/// (`CompIter::Range`'s three bounds; `CompIter::Name` holds none).
fn comp_iter_exprs<'a>(iter: &'a CompIter, exprs: &mut Vec<&'a HirExpr>) {
    match iter {
        CompIter::Range { start, stop, step } => {
            exprs.push(start);
            exprs.push(stop);
            exprs.push(step);
        }
        CompIter::Name(_) => {}
    }
}

fn reject_generic_calls_in_stmt(
    module_env: &Environment,
    own_name: &str,
    stmt: &HirStmt,
) -> Result<(), Diagnostic> {
    let mut exprs: Vec<&HirExpr> = Vec::new();
    let mut blocks: Vec<&[HirStmt]> = Vec::new();
    match stmt {
        HirStmt::ExprStmt(expr) | HirStmt::Assign { value: expr, .. } => exprs.push(expr),
        HirStmt::AnnAssign { value, .. } => exprs.extend(value.iter()),
        HirStmt::Return(value) => exprs.extend(value.iter()),
        HirStmt::If { test, body, orelse } => {
            exprs.push(test);
            blocks.push(body);
            blocks.push(orelse);
        }
        HirStmt::While { test, body } => {
            exprs.push(test);
            blocks.push(body);
        }
        HirStmt::ForRange {
            start,
            stop,
            step,
            body,
            ..
        } => {
            exprs.push(start);
            exprs.push(stop);
            exprs.push(step);
            blocks.push(body);
        }
        HirStmt::ForList { body, .. } => blocks.push(body),
        HirStmt::DictSet { key, value, .. } => {
            exprs.push(key);
            exprs.push(value);
        }
        HirStmt::AttrSet { base, value, .. } => {
            exprs.push(base);
            exprs.push(value);
        }
        HirStmt::ListCompAssign {
            iter, cond, elt, ..
        }
        | HirStmt::SetCompAssign {
            iter, cond, elt, ..
        } => {
            comp_iter_exprs(iter, &mut exprs);
            exprs.extend(cond.iter().map(|c| c.as_ref()));
            exprs.push(elt);
        }
        HirStmt::DictCompAssign {
            iter,
            cond,
            key,
            value,
            ..
        } => {
            comp_iter_exprs(iter, &mut exprs);
            exprs.extend(cond.iter().map(|c| c.as_ref()));
            exprs.push(key);
            exprs.push(value);
        }
    }
    for expr in exprs {
        reject_generic_calls_in_expr(module_env, own_name, expr)?;
    }
    for block in blocks {
        reject_generic_calls_in_block(module_env, own_name, block)?;
    }
    Ok(())
}

fn reject_generic_calls_in_expr(
    module_env: &Environment,
    own_name: &str,
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    match expr {
        HirExpr::Call { callee, args } => {
            if callee == own_name || module_env.lookup_generic(callee).is_some() {
                return Err(reject_generic_call(own_name, callee));
            }
            for arg in args {
                reject_generic_calls_in_expr(module_env, own_name, arg)?;
            }
            Ok(())
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            reject_generic_calls_in_expr(module_env, own_name, left)?;
            reject_generic_calls_in_expr(module_env, own_name, right)
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    reject_generic_calls_in_expr(module_env, own_name, inner)?;
                }
            }
            Ok(())
        }
        HirExpr::ListLiteral(elements)
        | HirExpr::SetLiteral(elements)
        | HirExpr::TupleLiteral(elements) => {
            for element in elements {
                reject_generic_calls_in_expr(module_env, own_name, element)?;
            }
            Ok(())
        }
        HirExpr::DictLiteral(pairs) => {
            for (key, value) in pairs {
                reject_generic_calls_in_expr(module_env, own_name, key)?;
                reject_generic_calls_in_expr(module_env, own_name, value)?;
            }
            Ok(())
        }
        HirExpr::Subscript { base, index } => {
            reject_generic_calls_in_expr(module_env, own_name, base)?;
            reject_generic_calls_in_expr(module_env, own_name, index)
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            reject_generic_calls_in_expr(module_env, own_name, base)?;
            for bound in [start, stop, step].into_iter().flatten() {
                reject_generic_calls_in_expr(module_env, own_name, bound)?;
            }
            Ok(())
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            reject_generic_calls_in_expr(module_env, own_name, value)
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            reject_generic_calls_in_expr(module_env, own_name, key)?;
            reject_generic_calls_in_expr(module_env, own_name, default)
        }
        HirExpr::AttrGet { base, .. } => reject_generic_calls_in_expr(module_env, own_name, base),
        HirExpr::MethodCall { base, args, .. } => {
            reject_generic_calls_in_expr(module_env, own_name, base)?;
            for arg in args {
                reject_generic_calls_in_expr(module_env, own_name, arg)?;
            }
            Ok(())
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. } => Ok(()),
    }
}

/// One successful D-134 call-site monomorphization: the concrete return
/// type the call site's own type-checking needs immediately, plus the
/// substituted, mangled, ordinary concrete-`Ty` function body that
/// `pycc_mir` (Task 3) registers exactly like a non-generic function.
///
/// `mangled_name` is keyed only by `(generic function name, type-parameter
/// name, concrete Ty)`, never by anything call-site-specific (argument
/// expressions, source span, call order) -- two call sites that resolve `T`
/// to the same concrete `Ty` therefore produce byte-for-byte the same
/// `mangled_name` and the same `specialized` body content, which is exactly
/// the key Task 3 needs to deduplicate multiple call sites into one
/// compiled specialization (D-134's own "two call sites, same concrete
/// type, one compiled specialization" requirement).
#[derive(Debug, Clone, PartialEq)]
pub struct GenericInstantiation {
    pub mangled_name: String,
    pub specialized: HirItem,
    pub return_ty: Ty,
}

/// Mangled-name scheme for one generic-function call-site instantiation
/// (D-133/D-134). `crates/pycc_mir/src` has no existing per-instantiation
/// function-name-mangling convention to reuse: D-105's own container
/// monomorphization (`list[int]`, `dict[str, int]`, `set[int]`, ...)
/// dispatches on `Ty` at codegen time through a fixed set of `pycc_rt`
/// runtime helper functions (see e.g. `crates/pycc_rt/src/lib.rs`), not by
/// generating a differently-named MIR/codegen function per instantiation --
/// there is no prior "list[int]'s specialized entry point" name to mirror,
/// only a set of runtime helper names that don't vary per call site at all
/// (plan-deviation note: the task brief asked to reuse an existing
/// convention; none exists, confirmed by inspection before writing this).
///
/// The scheme instead mirrors this same file's (`pycc_hir`)
/// `synthesize_comp_var_name` precedent for a synthesized identifier that
/// must never collide with a real user-source name: a real Python `NAME`
/// token can never start with a decimal digit (confirmed against the
/// vendored `ruff_python_parser` tokenizer, same fact `synthesize_comp_var_name`'s
/// own doc comment cites), so prefixing with `0gen_` guarantees this string
/// can never equal any function name lowering could have produced from real
/// source, no matter what the user names their own top-level functions --
/// unlike a merely unusual but still syntactically legal name (e.g. the
/// task brief's own illustrative `f__T_int`, which a user's own source
/// *could* spell literally, `__` is ordinary Python identifier syntax).
fn mangle_generic_instantiation(fn_name: &str, type_param_name: &str, concrete: &Ty) -> String {
    format!("0gen_{fn_name}__{type_param_name}_{}", concrete.name())
}

/// D-133: a `Ty`-tree rewrite substituting a top-level `Ty::Param(param_name)`
/// occurrence with `concrete`, leaving every other `Ty` shape untouched. Not
/// a *recursive* container walk: `generic_type_param_name` (called by every
/// public entry point before this function ever runs, both here and in
/// `check_generic_function`) already rejects any container-position
/// `Ty::Param` with `T0042`, so by the time `substitute_ty` runs, `ty` is
/// provably either a bare `Ty::Param(param_name)` or a shape that can never
/// contain one nested inside `List`/`Set`/`Dict`/`Tuple` -- recursing into
/// those container variants here would be untestable dead code (D-014's
/// hard coverage gate has no exemption for it), not genuine generality.
fn substitute_ty(ty: &Ty, param_name: &str, concrete: &Ty) -> Ty {
    match ty {
        Ty::Param(name) if name.as_ref() == param_name => concrete.clone(),
        other => other.clone(),
    }
}

/// D-133: clones a generic function's body, substituting only the `Ty`
/// annotations `substitute_ty` above would rewrite -- `HirStmt::AnnAssign`'s
/// `annotation` field is the only place an embedded `Ty` appears inside a
/// `HirStmt`/`HirExpr` tree (every `HirExpr` variant is untyped; type is
/// always inferred contextually), so every other statement shape is cloned
/// unchanged, recursing only into nested bodies (`If`/`While`/`ForRange`/
/// `ForList`) to reach a nested `AnnAssign`.
fn substitute_body(body: &[HirStmt], param_name: &str, concrete: &Ty) -> Vec<HirStmt> {
    body.iter()
        .map(|stmt| substitute_stmt(stmt, param_name, concrete))
        .collect()
}

fn substitute_stmt(stmt: &HirStmt, param_name: &str, concrete: &Ty) -> HirStmt {
    match stmt {
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty(annotation, param_name, concrete),
            value: value.clone(),
        },
        HirStmt::If { test, body, orelse } => HirStmt::If {
            test: test.clone(),
            body: substitute_body(body, param_name, concrete),
            orelse: substitute_body(orelse, param_name, concrete),
        },
        HirStmt::While { test, body } => HirStmt::While {
            test: test.clone(),
            body: substitute_body(body, param_name, concrete),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => HirStmt::ForRange {
            var: var.clone(),
            start: start.clone(),
            stop: stop.clone(),
            step: step.clone(),
            body: substitute_body(body, param_name, concrete),
        },
        HirStmt::ForList { var, list, body } => HirStmt::ForList {
            var: var.clone(),
            list: list.clone(),
            body: substitute_body(body, param_name, concrete),
        },
        other => other.clone(),
    }
}

/// D-133/D-134: resolves one call site's argument types against a generic
/// function's signature, substituting the single type parameter with the
/// one concrete `Ty` every occurrence agrees on, and produces the
/// specialized, mangled, ordinary concrete-`Ty` function body `pycc_mir`
/// (Task 3) registers as an independent function. Rejects with `T0042`
/// when: the function has no type parameter to instantiate; the call's own
/// arity doesn't match (reuses `T0021`'s existing call-arity message shape,
/// not a new code, mirroring `infer_expr_in`'s own `HirExpr::Call` arm);
/// two occurrences of the type parameter resolve to different concrete
/// `Ty`s; the type parameter is never used by any parameter (nothing to
/// resolve it from); or the resolved `Ty` is not one of
/// `Int`/`Float`/`Bool`/`Str` (D-134's own scalar-only call-site scope).
/// Every non-generic parameter position is still checked for ordinary
/// assignability via the existing `is_assignable`, reusing `T0021`'s
/// existing "argument N expects ..." message shape exactly (not a new
/// diagnostic for an unrelated failure mode).
pub fn instantiate_generic_call(
    func: &HirItem,
    arg_tys: &[Ty],
) -> Result<GenericInstantiation, Diagnostic> {
    let HirItem::Function {
        name,
        params,
        return_ty,
        body,
    } = func
    else {
        panic!("instantiate_generic_call called with a non-Function HirItem");
    };
    let type_param_name = generic_type_param_name(params, return_ty)?.ok_or_else(|| {
        t0042(format!(
            "`{name}` has no PEP 695 type parameter to instantiate"
        ))
    })?;
    if arg_tys.len() != params.len() {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{name}` expects {} argument(s), got {}",
                params.len(),
                arg_tys.len()
            ),
            Span::new(0, 0),
        ).with_help(format!("pass exactly {} argument(s)", params.len())));
    }
    let mut resolved: Option<Ty> = None;
    for ((param_name, param_ty), arg_ty) in params.iter().zip(arg_tys) {
        match param_ty {
            Ty::Param(name_ref) if name_ref.as_str() == type_param_name => match &resolved {
                Some(existing) if existing != arg_ty => {
                    return Err(t0042(format!(
                        "generic function `{name}`'s type parameter `{type_param_name}` was resolved inconsistently across its own call-site arguments (`{}` vs `{}`)",
                        existing.name(),
                        arg_ty.name()
                    )));
                }
                _ => resolved = Some(arg_ty.clone()),
            },
            other => {
                if !is_assignable(arg_ty.clone(), other.clone()) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "argument `{param_name}` of `{name}` expects `{}`, got `{}`",
                            other.name(),
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("pass a `{}` value", other.name())));
                }
            }
        }
    }
    let concrete = resolved.ok_or_else(|| {
        t0042(format!(
            "generic function `{name}`'s type parameter `{type_param_name}` is not used by any parameter, so no call-site argument can resolve it"
        ))
    })?;
    if !matches!(concrete, Ty::Int | Ty::Float | Ty::Bool | Ty::Str) {
        return Err(t0042(format!(
            "generic function `{name}` was called with `{}` for type parameter `{type_param_name}`, but v0.2 only instantiates a type parameter with `int`, `float`, `bool`, or `str`",
            concrete.name()
        )));
    }
    let substituted_params = params
        .iter()
        .map(|(param_name, ty)| {
            (
                param_name.clone(),
                substitute_ty(ty, &type_param_name, &concrete),
            )
        })
        .collect();
    let substituted_return = substitute_ty(return_ty, &type_param_name, &concrete);
    let substituted_body = substitute_body(body, &type_param_name, &concrete);
    let mangled_name = mangle_generic_instantiation(name, &type_param_name, &concrete);
    Ok(GenericInstantiation {
        mangled_name: mangled_name.clone(),
        return_ty: substituted_return.clone(),
        specialized: HirItem::Function {
            name: mangled_name,
            params: substituted_params,
            return_ty: substituted_return,
            body: substituted_body,
        },
    })
}

/// PR-13 Task 3 (D-133/D-134): rewrites every call site whose callee is a
/// PEP 695 generic function into a call to that call site's mangled,
/// monomorphized specialization, mutating `expr` and any nested
/// sub-expression in place. Returns `expr`'s resulting `Ty` exactly like
/// `infer_expr_in` would (the two agree on every non-generic-call
/// expression, since this function delegates directly to `infer_expr_in`
/// once every nested `HirExpr::Call` reachable from `expr` has already been
/// rewritten to name only ordinary, concrete functions).
///
/// This does not repeat `infer_expr_in`'s own validation logic -- `env` is
/// only ever used here to look up a call's argument types via the ordinary,
/// unmodified `infer_expr_in`/`instantiate_generic_call`, both of which
/// this module's callers have already run successfully once (D-133/D-134's
/// call-arm dispatch in `infer_expr_in` itself) before this rewrite ever
/// runs. Every `HirExpr` variant with a sub-expression is visited so a
/// generic call nested at any depth (e.g. `print(identity(1))`) is found
/// and rewritten; a leaf variant has nothing to recurse into and falls
/// straight through to `infer_expr_in`.
fn rewrite_generic_calls_in_expr(
    env: &mut Environment,
    local_names: &[&str],
    expr: &mut HirExpr,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::Call { callee, args } => {
            // Each arg's `Ty` comes directly from this same rewriting
            // recursion's own return value -- not a second, separate
            // `infer_expr_in` pass over the now-rewritten args -- since
            // both would always agree (this function delegates to
            // `infer_expr_in` for exactly the types it doesn't special-case
            // itself) and only one needs to actually run.
            let arg_tys = args
                .iter_mut()
                .map(|arg| {
                    rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(generic_func) = env.lookup_generic(callee).cloned() {
                let instantiation = instantiate_generic_call(&generic_func, &arg_tys)?;
                // Registered into `env` regardless of whether this is the
                // first call site to discover this exact specialization --
                // this scope's own `env` may be an independent clone (e.g.
                // a fresh `child_for_function` per function) that never
                // observed a sibling scope's earlier registration, and a
                // *later* expression in this same scope may itself call the
                // now-rewritten mangled name (directly, or as a nested
                // generic-call argument), which needs it bound to resolve
                // as an ordinary function via `infer_expr_in`.
                if env.lookup_function(&instantiation.mangled_name).is_none() {
                    env.bind_function(
                        instantiation.mangled_name.clone(),
                        arg_tys,
                        instantiation.return_ty.clone(),
                    );
                }
                if seen.insert(instantiation.mangled_name.clone()) {
                    instantiations.push(instantiation.clone());
                }
                *callee = instantiation.mangled_name;
                return Ok(instantiation.return_ty);
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            for sub in [left.as_mut(), right.as_mut()] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::FString(parts) => {
            for part in parts.iter_mut() {
                if let FStringPart::Interpolation(inner) = part {
                    rewrite_generic_calls_in_expr(env, local_names, inner, instantiations, seen)?;
                }
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::ListLiteral(elements)
        | HirExpr::SetLiteral(elements)
        | HirExpr::TupleLiteral(elements) => {
            for element in elements.iter_mut() {
                rewrite_generic_calls_in_expr(env, local_names, element, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::DictLiteral(pairs) => {
            for (key, value) in pairs.iter_mut() {
                for sub in [key, value] {
                    rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
                }
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::Subscript { base, index } => {
            for sub in [base.as_mut(), index.as_mut()] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            for sub in std::iter::once(base.as_mut()).chain(
                [start, stop, step]
                    .into_iter()
                    .flatten()
                    .map(|b| b.as_mut()),
            ) {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            for sub in [key.as_mut(), default.as_mut()] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::AttrGet { base, .. } => {
            rewrite_generic_calls_in_expr(env, local_names, base, instantiations, seen)?;
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::MethodCall { base, args, .. } => {
            rewrite_generic_calls_in_expr(env, local_names, base, instantiations, seen)?;
            for arg in args.iter_mut() {
                rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. } => infer_expr_in(env, local_names, expr),
    }
}

/// Structural counterpart to `rewrite_generic_calls_in_expr`, one level up:
/// walks every statement position, rewriting any embedded `HirExpr` in
/// place and growing `env`'s bindings exactly enough (a plain `env.bind`,
/// not the validating `check_assignment`) for a later statement in the same
/// scope to resolve a local name as a generic call's argument. No shape or
/// assignability validation happens here -- the module already passed that
/// validation once, via the ordinary `check`/`check_and_resolve` path that
/// calls this function only afterward (see `monomorphize`).
fn rewrite_generic_calls_in_stmt(
    env: &mut Environment,
    local_names: &[&str],
    stmt: &mut HirStmt,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::ExprStmt(expr) => {
            rewrite_generic_calls_in_expr(env, local_names, expr, instantiations, seen)?;
            Ok(())
        }
        HirStmt::Assign { target, value } => {
            let ty = rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            env.bind(target.clone(), ty);
            Ok(())
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
        } => {
            if let Some(value) = value {
                rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            }
            env.bind(target.clone(), annotation.clone());
            Ok(())
        }
        HirStmt::If { test, body, orelse } => {
            rewrite_generic_calls_in_expr(env, local_names, test, instantiations, seen)?;
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            for s in orelse.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            rewrite_generic_calls_in_expr(env, local_names, test, instantiations, seen)?;
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            for sub in [start, stop, step] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(var.clone(), Ty::Int);
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            // Issue #118 Part 1: use `lookup_any` (not `lookup`) since this
            // pass runs post-validation and needs the type regardless of
            // binding state -- the validation pass already rejected any
            // maybe-bound iterable read.
            let var_ty = match env.lookup_any(list) {
                Some(Ty::List(elem)) => *elem,
                Some(Ty::Dict(kv)) => kv.0,
                Some(Ty::Set(elem)) => *elem,
                _ => {
                    // Already validated as iterable by the ordinary check
                    // pass that ran before `monomorphize`; a scalar or
                    // missing binding here would mean that validation was
                    // skipped, which no public entry point allows.
                    Ty::Infer
                }
            };
            env.bind(var.clone(), var_ty);
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::DictSet { key, value, .. } => {
            for sub in [key, value] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::AttrSet { base, value, .. } => {
            for sub in [base, value] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = rewrite_comp_iter(env, local_names, iter, instantiations, seen)?;
            env.bind(var.clone(), var_ty);
            for sub in cond
                .iter_mut()
                .map(|c| c.as_mut())
                .chain(std::iter::once(elt.as_mut()))
            {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(target.clone(), Ty::List(Box::new(Ty::Int)));
            Ok(())
        }
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = rewrite_comp_iter(env, local_names, iter, instantiations, seen)?;
            env.bind(var.clone(), var_ty);
            for sub in cond
                .iter_mut()
                .map(|c| c.as_mut())
                .chain(std::iter::once(elt.as_mut()))
            {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(target.clone(), Ty::Set(Box::new(Ty::Int)));
            Ok(())
        }
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let var_ty = rewrite_comp_iter(env, local_names, iter, instantiations, seen)?;
            env.bind(var.clone(), var_ty);
            for sub in cond
                .iter_mut()
                .map(|c| c.as_mut())
                .chain([key.as_mut(), value.as_mut()])
            {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(target.clone(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
            Ok(())
        }
        HirStmt::Return(value) => {
            if let Some(value) = value {
                rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            }
            Ok(())
        }
    }
}

/// `CompIter`'s own rewrite counterpart -- mirrors `resolve_comp_iter`
/// exactly (same three iterable shapes, same resulting loop-variable `Ty`),
/// but also rewrites any generic call reachable from a `CompIter::Range`
/// bound, which `resolve_comp_iter` (a read-only helper reused as-is
/// elsewhere in this file) has no reason to do.
fn rewrite_comp_iter(
    env: &mut Environment,
    local_names: &[&str],
    iter: &mut CompIter,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<Ty, Diagnostic> {
    match iter {
        CompIter::Range { start, stop, step } => {
            for sub in [start, stop, step] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            Ok(Ty::Int)
        }
        CompIter::Name(name) => match env.lookup_any(name) {
            Some(Ty::List(elem)) => Ok(*elem),
            Some(Ty::Dict(kv)) => Ok(kv.0),
            Some(Ty::Set(elem)) => Ok(*elem),
            // Already validated as iterable before `monomorphize` ever
            // runs; see `ForList`'s own fallback above.
            _ => Ok(Ty::Infer),
        },
    }
}

/// PR-13 Task 3 (D-133/D-134): the monomorphization pass that turns a
/// validated module possibly containing PEP 695 generic functions into an
/// equivalent module containing only ordinary, concrete-`Ty` functions --
/// exactly what `pycc_mir::build` already expects, with no MIR/codegen
/// change needed for genericity itself. Every call site whose callee was a
/// generic function is rewritten to call that call site's mangled,
/// monomorphized specialization instead; two call sites resolving the same
/// type parameter to the same concrete `Ty` share one specialization
/// (deduplicated by `GenericInstantiation::mangled_name`, per
/// `instantiate_generic_call`'s own documented dedup guarantee). Every
/// original generic function itself is dropped -- it still carries
/// `Ty::Param`, which must never reach `pycc_mir` -- and each distinct
/// specialization is appended as an ordinary `HirItem::Function`, in the
/// order its first call site was encountered (module item order, depth
/// first through each function body).
///
/// Callers must have already validated `hir` (e.g. via `check` or
/// `checked_function_signatures`, both of which now dispatch a generic call
/// site through `instantiate_generic_call` themselves via
/// `infer_expr_in`'s own generic arm) -- this pass performs no validation
/// of its own and instead trusts that every generic call site it encounters
/// resolves successfully.
fn monomorphize(hir: &HirModule) -> Result<HirModule, Diagnostic> {
    let generics: HashMap<String, HirItem> = hir
        .items
        .iter()
        .filter_map(|item| match item {
            HirItem::Function {
                name,
                params,
                return_ty,
                ..
            } if is_generic_signature(params, return_ty) => Some((name.clone(), item.clone())),
            _ => None,
        })
        .collect();
    if generics.is_empty() {
        // PR-13 final review (I1): `type_aliases` is emptied here exactly as
        // it is on the monomorphized path below, so a resolved module's own
        // `type_aliases` value never depends on whether the module happened
        // to contain a generic function. D-135 aliases are fully discharged
        // during HIR lowering (`annotation_to_ty` resolves every alias name
        // into a concrete `Ty` before `pycc_types` runs); nothing downstream
        // -- not `pycc_mir`, not `pycc_codegen` -- reads the field after
        // lowering, so the resolved HIR's `type_aliases` is empty by design.
        return Ok(HirModule {
            items: hir.items.clone(),
            type_aliases: Vec::new(),
            imports: Vec::new(),
            // Unlike `type_aliases`/`imports` (both fully discharged during
            // HIR lowering -- nothing downstream reads either again),
            // `class_defs` is actively consumed after this point: `check`'s
            // own class-body checking (Task 3) and every one of
            // `pycc_mir`/`pycc_codegen`'s slot-index/method-mangled-name
            // lookups (Tasks 5/6) read it from the `HirModule` that reaches
            // them, which is this function's own return value on the
            // monomorphized path. Dropping it here would silently break
            // every class-containing module the moment it reached
            // `pycc_mir::build`.
            class_defs: hir.class_defs.clone(),
        });
    }

    let mut env = Environment::new();
    // D-154 (Part 1 of #375): this pass's own `env` needs the class table
    // too -- `rewrite_generic_calls_in_expr`'s `AttrGet`/`MethodCall` arms
    // fall through to `infer_expr_in` exactly like every other arm here,
    // which resolves a class-instance attribute/method through
    // `Environment::classes` (`class::resolve_attr_get`/
    // `resolve_method_call`). Without this, a generic function whose own
    // body reads/writes an instance attribute or calls a method would
    // panic here with "class `X` has no registered HirClassDef" the
    // moment this pass reached it, even though `check_with_signatures`
    // already validated that exact body successfully against a `classes`-
    // populated `Environment` of its own.
    class::bind_classes(&mut env, hir);
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        {
            let param_tys = params.iter().map(|(_, ty)| ty.clone()).collect::<Vec<_>>();
            env.bind_function(name.clone(), param_tys, return_ty.clone());
        }
    }
    for (name, item) in &generics {
        env.bind_generic(name.clone(), item.clone());
    }

    let function_local_names = module_function_local_names(hir);
    let mut instantiations: Vec<GenericInstantiation> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Two passes, matching `check_with_environment`'s own D-041 discipline
    // exactly (register every function's signature -- already done above --
    // then process every top-level statement in source order against the
    // *growing* module `env`, then check every function body against the
    // module `env` as it stands once the whole module's top-level code has
    // run): a function body must be able to rewrite a generic call whose
    // argument reads a module-level global assigned *later* in the file
    // than the function's own `def`, exactly as `check_function_in` already
    // allows for ordinary type-checking. A single source-order pass here
    // would process a function body's rewrite before a later top-level
    // assignment had grown `env`, wrongly reporting the global as
    // undefined even though `check`/`check_and_resolve`'s own validation
    // (which already ran successfully before `monomorphize` is ever called)
    // accepted the exact same program.
    //
    // Each item's *original* index is carried alongside its rewritten form
    // so the final assembly can restore source order -- this pass processes
    // top-level statements and function bodies in two separate sweeps, not
    // in original item order.
    let mut rewritten: Vec<Option<HirItem>> = vec![None; hir.items.len()];

    // Pass 1: every top-level statement, in source order, growing `env`.
    for (index, (item, local_names)) in hir.items.iter().zip(&function_local_names).enumerate() {
        if let HirItem::TopLevelStmt(stmt) = item {
            let mut new_stmt = stmt.clone();
            rewrite_generic_calls_in_stmt(
                &mut env,
                local_names,
                &mut new_stmt,
                &mut instantiations,
                &mut seen,
            )?;
            rewritten[index] = Some(HirItem::TopLevelStmt(new_stmt));
        }
    }

    // Pass 2: every function body, against the module `env` as it stands
    // once the whole module's top-level code has been processed above --
    // an original generic function is dropped entirely (only its concrete
    // specializations, appended below, may reach `pycc_mir`).
    for (index, (item, local_names)) in hir.items.iter().zip(&function_local_names).enumerate() {
        let HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } = item
        else {
            continue;
        };
        if generics.contains_key(name) {
            continue;
        }
        let mut fn_env = env.child_for_function(local_names);
        for (param_name, param_ty) in params {
            fn_env.bind(param_name.clone(), param_ty.clone());
        }
        let mut new_body = body.clone();
        for stmt in new_body.iter_mut() {
            rewrite_generic_calls_in_stmt(
                &mut fn_env,
                local_names,
                stmt,
                &mut instantiations,
                &mut seen,
            )?;
        }
        rewritten[index] = Some(HirItem::Function {
            name: name.clone(),
            params: params.clone(),
            return_ty: return_ty.clone(),
            body: new_body,
        });
    }

    let mut items = rewritten.into_iter().flatten().collect::<Vec<_>>();
    for instantiation in instantiations {
        items.push(instantiation.specialized);
    }
    // `type_aliases`/`imports` are empty by design on both of this
    // function's exits -- see the no-generics early return above (PR-13
    // final review I1) and that return's own comment for why `class_defs`
    // is not treated the same way.
    Ok(HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: hir.class_defs.clone(),
    })
}

/// Type-checks a module and returns a cloned HIR whose function signatures
/// contain only the concrete types resolved by private-helper inference.
/// Consumers after the type boundary must use this module rather than the
/// unresolved lowering result so `Ty::Infer` can never leak into MIR or code
/// generation. PR-13 Task 3 (D-133/D-134): also monomorphizes every PEP 695
/// generic function call site into a call to a concrete, mangled
/// specialization (see `monomorphize`) -- the returned module contains only
/// ordinary concrete-`Ty` functions, exactly what `pycc_mir::build` expects.
pub fn check_and_resolve(hir: &HirModule) -> Result<HirModule, Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    let signatures = checked_function_signatures(hir, &function_local_names)?;

    let mut resolved_hir = hir.clone();
    for item in &mut resolved_hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        let (resolved_params, resolved_return) = signatures
            .get(name)
            .expect("every HIR function received an inferred signature");
        for ((_, param_ty), resolved_ty) in params.iter_mut().zip(resolved_params) {
            *param_ty = resolved_ty.clone();
        }
        *return_ty = resolved_return.clone();
    }

    // Issue #22/#402: no post-resolution redefinition recheck is needed
    // here. `checked_function_signatures` (called above) already runs
    // `check_incompatible_redefinitions` pre-resolution, and that function
    // now compares the full raw shape unconditionally, including any
    // `Ty::Infer` position (see its own doc comment) -- so every
    // redefinition pair that reaches this point is already raw-shape-
    // identical and is guaranteed to resolve to the same concrete
    // signature. A second check here would be unreachable dead code.
    monomorphize(&resolved_hir)
}

fn check_with_signatures(
    hir: &HirModule,
    signatures: &HashMap<String, (Vec<Ty>, Ty)>,
    function_local_names: &[Vec<&str>],
) -> Result<(), Diagnostic> {
    let mut env = Environment::new();
    // D-154 (Part 1 of #375): register every declared class before
    // checking any statement body -- a class must be usable (instantiated,
    // its instances passed around) from anywhere in the module, the same
    // "visible regardless of source position" requirement functions
    // already get from pass 1 below.
    class::bind_classes(&mut env, hir);
    // Pass 1: register every function's signature before checking any
    // statement body, matching Python's own "a module runs top to bottom,
    // but any def already executed is callable" semantics -- top-level
    // code and other function bodies (D-040) both need to see every
    // function regardless of its position in the file.
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty: item_return_ty,
            ..
        } = item
        {
            let (param_tys, return_ty) = signatures
                .get(name)
                .expect("every HIR function received an inferred signature");
            // D-133/D-134: a generic function's *original* body (still
            // carrying `Ty::Param`) is registered separately so a call
            // site can be resolved via `instantiate_generic_call` --
            // `signatures` itself already carries the same `Ty::Param`
            // entries (never `Ty::Infer`, so it survives both the
            // concrete-fast-path and solver-inferred paths unchanged), but
            // has no room for the body substitution needs.
            if is_generic_signature(params, item_return_ty) {
                env.bind_generic(name.clone(), item.clone());
            }
            env.bind_function(name.clone(), param_tys.clone(), return_ty.clone());
        }
    }
    check_with_environment(hir, env, function_local_names)
}

/// Issue #22: reject a redefinition of the same function name with an
/// incompatible signature. The codegen uses the first definition's LLVM
/// function type for indirect calls through the per-name function-pointer
/// slot, so all definitions of the same name must share one signature.
/// CPython allows arbitrary redefinition, but pycc's v0.1/v0.2 scope does
/// not need to support it, and the pre-#22 behavior was already broken
/// (a compile-time LLVM verification failure or silent runtime UB). This
/// check makes the "one signature per name" invariant the codegen already
/// assumes actually true.
///
/// Compares each definition's raw, pre-resolution `(param_tys, return_ty)`
/// shape via `Ty`'s derived structural `PartialEq` -- including any
/// `Ty::Infer` position. `Ty::Infer` is an ordinary unit variant under that
/// derive, so it only ever equals another `Ty::Infer` at the same position,
/// never a concrete type: comparing it unconditionally is correct by
/// construction, not a false positive. An earlier version of this function
/// skipped any function whose signature contained `Ty::Infer`, reasoning
/// that comparing an unresolved `Infer` against a concrete type would be
/// unsound -- that reasoning was wrong (the derived `PartialEq` already
/// handles it correctly) and the skip instead let a same-named,
/// same-arity, `Ty::Infer`-vs-concrete redefinition collapse onto one
/// shared signature and pass silently (issue #402). Do not reintroduce a
/// skip for `Ty::Infer` positions.
///
/// Called from `check` and `checked_function_signatures` (catches the
/// concrete path before the concrete/solver split), both pre-resolution.
/// `check_and_resolve` no longer needs its own post-resolution recheck:
/// since this function now rejects every raw-shape mismatch up front
/// (arity or `Infer`-vs-concrete), any redefinition pair it accepts is
/// already raw-shape-identical, and `infer_function_signatures_with_solver`
/// resolves same-named items through one shared, name-keyed signature
/// entry -- so raw-shape-identical items are guaranteed to resolve to the
/// same concrete signature too. A post-resolution recheck would therefore
/// never observe a case this pre-resolution check didn't already reject.
fn check_incompatible_redefinitions(hir: &HirModule) -> Result<(), Diagnostic> {
    let mut seen: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        let current = (
            params.iter().map(|(_, ty)| ty.clone()).collect::<Vec<_>>(),
            return_ty.clone(),
        );
        if let Some(prev) = seen.get(name) {
            if prev != &current {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot redefine function `{name}` with a different signature \
                         (previous: {}, current: {})",
                        format_function_signature(prev),
                        format_function_signature(&current),
                    ),
                    Span::new(0, 0),
                ));
            }
        } else {
            seen.insert(name.clone(), current);
        }
    }
    Ok(())
}

/// Formats a `(param_tys, return_ty)` pair as `def name(params) -> return`
/// for diagnostic messages.
fn format_function_signature(sig: &(Vec<Ty>, Ty)) -> String {
    let params = sig
        .0
        .iter()
        .map(|ty| ty.name())
        .collect::<Vec<_>>()
        .join(", ");
    format!("({params}) -> {}", sig.1.name())
}

fn check_with_environment(
    hir: &HirModule,
    mut env: Environment,
    function_local_names: &[Vec<&str>],
) -> Result<(), Diagnostic> {
    // Issue #22: clear `defined_functions` before the top-level source-order
    // pass. `bind_function` (called by `check_with_signatures`'s pass 1 or
    // `concrete_function_environment`) adds every function to this set, but
    // for top-level checking we need to track which `def`s have actually
    // been *executed* in source order -- a call before the `def`'s position
    // is a NameError in CPython and must be rejected here. Function bodies
    // (pass 3) get a fresh seed of all function names via
    // `child_for_function`, so they're unaffected.
    env.defined_functions.clear();
    // Pass 2: check every top-level statement in source order, growing
    // `env`'s bindings as module-level assignments are encountered --
    // ordinary top-level code is still checked top-to-bottom (a top-level
    // forward reference to a not-yet-assigned name is a genuine error).
    // A `def` executes at its own position in that order and rebinds its
    // name to the function (D-110, refined by PR #252's review): later
    // `helper()` calls resolve the function exactly as CPython does, while
    // a value assignment *after* the `def` shadows it again. The `def` only
    // marks the name def-rebound -- it must NOT erase the representation
    // record in `bindings`, which D-040's sticky-representation rule keeps
    // consulting so an incompatible later reassignment still fails T0023.
    // The gate therefore tests the net source-order binding, in this pass
    // and in pass 3's final environment alike.
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => check_stmt(&mut env, stmt)?,
            HirItem::Function { name, .. } => {
                env.def_rebound.insert(name.clone());
                // Issue #22: a `def` at its source position makes the
                // function name callable from this point forward in
                // top-level code. Calls before this point (the name is
                // not yet in `defined_functions`) are rejected by
                // `infer_expr_in`'s `HirExpr::Call` arm.
                env.defined_functions.insert(name.clone());
            }
        }
    }
    // Pass 3: check every function body against a clone of `env` as it
    // stands once the whole module's top-level code has been processed
    // (D-041) -- a function can read any module-level global regardless of
    // whether its own `def` appears before or after that global's
    // assignment in the file, since real Python only evaluates a function
    // body when it's *called*, typically after the module has finished
    // running top to bottom.
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        if let HirItem::Function {
            params, return_ty, ..
        } = item
        {
            // D-133/D-134: a generic function's body is checked through
            // `check_generic_function_in` -- the shape gate, the Critical
            // self/mutual-generic-recursion rejection, and then the same
            // ordinary sibling-aware `check_function_in` body check every
            // non-generic function gets (PR-13 final review I3: the earlier
            // env-less variant could not see any sibling function, so a
            // generic body calling an ordinary sibling wrongly reported
            // "call to undefined function").
            if is_generic_signature(params, return_ty) {
                check_generic_function_in(&env, item, local_names)?;
            } else {
                check_function_in(&env, item, local_names)?;
            }
        }
    }
    Ok(())
}

/// Type-checks a module without materializing a resolved HIR clone.
///
/// Use [`check_and_resolve`] when a downstream compiler stage needs concrete
/// private-helper signatures in the returned HIR.
pub fn check(hir: &HirModule) -> Result<(), Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    // Issue #22: reject incompatible redefinitions before trying either the
    // concrete or solver path -- including a same-arity, `Ty::Infer`-
    // involving mismatch (see `check_incompatible_redefinitions`'s own doc
    // comment). Calling it here (not inside `check_with_environment`)
    // ensures the error is returned directly rather than being masked by
    // the concrete-path fallback to the solver path.
    check_incompatible_redefinitions(hir)?;
    // The public validation-only API has no resolved-signature result to
    // return. Avoid building a temporary concrete signature map and then
    // cloning it into an `Environment`: construct that environment directly.
    // On validation failure, preserve the historical solver-first diagnostic
    // selection exactly as `checked_function_signatures` does.
    if let Some(env) = concrete_function_environment(hir)
        && check_with_environment(hir, env, &function_local_names).is_ok()
    {
        return Ok(());
    }
    let signatures = infer_function_signatures_with_solver(hir, &function_local_names)?;
    check_with_signatures(hir, &signatures, &function_local_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_1_slice_always_type_checks() {
        let hir = HirModule {
            items: vec![],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn concrete_signatures_take_the_validation_only_fast_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "identity".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert_eq!(
            concrete_function_signatures(&hir).unwrap()["identity"],
            (vec![Ty::Int], Ty::Int)
        );
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn inferred_signatures_keep_the_constraint_solver_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_identity".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(concrete_function_signatures(&hir).is_none());
    }

    #[test]
    fn concrete_fast_path_preserves_solver_first_diagnostic_selection() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "takes_int".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "broken".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "takes_int".to_string(),
                            args: vec![HirExpr::StringLiteral("wrong".to_string())],
                        }),
                        HirStmt::Return(Some(HirExpr::StringLiteral("wrong".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let concrete = concrete_function_signatures(&hir).unwrap();
        let validation_first = check_with_signatures(&hir, &concrete, &local_names).unwrap_err();
        let solver_first = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        let fast_path = checked_function_signatures(&hir, &local_names).unwrap_err();
        let public_check = check(&hir).unwrap_err();

        assert_eq!(validation_first.code, "T0021");
        assert_eq!(solver_first.code, "T0022");
        assert_eq!(fast_path.code, solver_first.code);
        assert_eq!(fast_path.message, solver_first.message);
        assert_eq!(public_check.code, solver_first.code);
        assert_eq!(public_check.message, solver_first.message);
    }

    #[test]
    fn constraint_collection_rejects_bound_and_unbound_local_call_targets() {
        for (body, expected_message) in [
            (
                vec![
                    HirStmt::Assign {
                        target: "helper".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::ExprStmt(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }),
                ],
                "name `helper` is bound to a non-callable value",
            ),
            (
                vec![
                    HirStmt::ExprStmt(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }),
                    HirStmt::Assign {
                        target: "helper".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                ],
                "local name `helper` is not bound before this use",
            ),
        ] {
            let hir = HirModule {
                items: vec![HirItem::Function {
                    name: "_caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body,
                }],
                type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
            };

            assert_eq!(check(&hir).unwrap_err().message, expected_message);
        }
    }

    #[test]
    fn constraint_collection_skips_already_concrete_call_arguments() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "takes_int".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "_caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "takes_int".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn constraint_collection_reuses_a_top_level_for_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "item".to_string(),
                    value: HirExpr::IntLiteral(0),
                }),
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn constraint_collection_rejects_a_non_integer_top_level_for_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "item".to_string(),
                    value: HirExpr::StringLiteral("not an integer".to_string()),
                }),
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_list_literal_still_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path()
     {
        // Any `Ty::Infer` signature anywhere in the module routes `check`
        // through `infer_function_signatures_with_solver` first (see
        // `checked_function_signatures`), which runs `collect_expr_constraints`
        // over every expression in the module, including this list literal.
        // That solver-side pass must stay lenient (`Ok(None)`, recurse only)
        // for list forms -- confirms it doesn't wrongly reject a valid list.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_heterogeneous_list_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The load-bearing counterpart to the test above: the solver's
        // leniency must not swallow a genuine list error -- it has to fall
        // through to the real, list-aware check pass (`check_with_signatures`)
        // that runs after the solver, which is what actually raises T0032.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::StringLiteral("two".to_string()),
                    ]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0032");
    }

    #[test]
    fn a_for_list_loop_still_type_checks_correctly_when_the_solver_path_runs_first() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "i".to_string(),
                    list: "xs".to_string(),
                    body: vec![],
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    // PR-12 Task 7 (D-118): whole-module `check()` tests for `HirExpr::Slice`,
    // exercising the real statement walkers (`check_with_environment`'s
    // `check_stmt`, and `collect_block_constraints`'s `Assign` arm under the
    // solver path) rather than calling `infer_expr`/`collect_expr_constraints`
    // directly on a hand-built expression. Mirrors the list-literal pair
    // immediately above: Task 3's own ledger records a real regression of
    // exactly this shape (a solver arm that looked correct in isolation but
    // was never actually reached by the block walker) -- these confirm the
    // `Slice` arm added in this task is genuinely wired into both paths, not
    // just correct when invoked directly.

    #[test]
    fn slicing_type_checks_through_the_full_check_pipeline_on_the_fast_path() {
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
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: Some(Box::new(HirExpr::IntLiteral(1))),
                        stop: None,
                        step: None,
                    },
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn slicing_a_list_of_str_is_rejected_through_the_full_check_pipeline() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: None,
                        stop: None,
                        step: None,
                    },
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0034");
    }

    #[test]
    fn slicing_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path() {
        // Companion to `a_list_literal_still_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path`
        // above: proves `collect_block_constraints`'s ordinary `Assign` arm
        // reaches this task's new `HirExpr::Slice` constraint-collection arm
        // (which must stay lenient, `Ok(None)`, recursing only) without
        // wrongly rejecting a valid slice, and that the real check pass
        // (`check_with_signatures`) run afterward still type-checks it.
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
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: Some(Box::new(HirExpr::IntLiteral(1))),
                        stop: Some(Box::new(HirExpr::IntLiteral(3))),
                        step: None,
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn slicing_a_list_of_str_is_still_rejected_when_the_solver_path_runs_first() {
        // Load-bearing counterpart: the solver's leniency for `Slice` must
        // not swallow a genuine `T0034` -- it has to fall through to the
        // real, list-aware check pass that runs after the solver.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: None,
                        stop: None,
                        step: None,
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0034");
    }

    #[test]
    fn collect_block_constraints_binds_the_annotation_and_records_the_initializer_default() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &[],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(5)),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert_eq!(env.bindings.get("y"), Some(&Ok(Ty::Int)));
        assert_eq!(
            constraints.annotation_defaults,
            vec![AnnotationDefaultConstraint {
                initializer: Ok(Ty::Int),
                annotation: Ty::Int,
            }]
        );
    }

    #[test]
    fn collect_block_constraints_preserves_an_existing_annotated_target_binding() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
            bindings: HashMap::from([("y".to_string(), Ok(Ty::Str))]),
            local_names: &["y"],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Str,
            value: Some(HirExpr::StringLiteral("again".to_string())),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert_eq!(env.bindings.get("y"), Some(&Ok(Ty::Str)));
    }

    #[test]
    fn collect_block_constraints_binds_the_annotation_when_the_initializer_has_no_term() {
        // `unresolved_global` is neither already bound nor a declared local of
        // this scope, so `collect_expr_constraints` returns `Ok(None)` for it
        // (the same "punt, nothing to unify yet" case a plain `Assign` to an
        // unresolved global would hit). The new `AnnAssign` arm cannot record
        // an initializer default without a term, but the annotation still
        // gives `y` a concrete solver binding.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &[],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::Name("unresolved_global".to_string())),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert_eq!(env.bindings.get("y"), Some(&Ok(Ty::Int)));
        assert!(constraints.annotation_defaults.is_empty());
    }

    #[test]
    fn collect_block_constraints_ignores_a_value_less_annotated_assignment() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &["y"],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: None,
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(!env.bindings.contains_key("y"));
        assert!(constraints.annotation_defaults.is_empty());
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_the_initializer_expression() {
        // `z` is declared local to this scope but never bound, so
        // `collect_expr_constraints` returns `Err(unbound_local)` for it --
        // the `?` inside the new arm must propagate that error rather than
        // being reachable only via the `Some`/`None` term paths above.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &["z"],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::Name("z".to_string())),
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_carries_a_homogeneous_scalar_list_literal_as_an_element_type_carrier() {
        // D-146 (#239): a homogeneous scalar-element list literal now produces
        // `Some(Ok(Ty::List(...)))` as a destructured element-type carrier --
        // never unified, only destructured by the `Subscript`/`ListPop` arms
        // to extract the scalar element type for scalar return-type inference.
        // Exact `Ty` equality (not `merge_inferred_types`) determines
        // homogeneity, matching `infer_expr_in`'s own rule.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::List(Box::new(Ty::Int)))));
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_list_literal_element() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![HirExpr::Name("missing".to_string())]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_subscript_as_unconstrained_but_recurses_into_base_and_index()
    {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::IntLiteral(1)),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_subscript_base() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("missing".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_subscript_index() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::IntLiteral(1)),
            index: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_carries_a_homogeneous_float_list_literal_as_an_element_type_carrier() {
        // D-146 (#239): a homogeneous `float`-element list literal produces
        // `Some(Ok(Ty::List(Box::new(Ty::Float))))`, proving the carrier is
        // generic over any private-solver scalar, not `Ty::Int`-specific.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::FloatLiteral(1.0),
            HirExpr::FloatLiteral(2.0),
        ]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::List(Box::new(Ty::Float)))));
    }

    #[test]
    fn constraint_collection_carries_a_single_element_scalar_list_literal() {
        // D-146 (#239): a single-element list is trivially homogeneous -- the
        // carrier is produced for arity 1 just as for arity 2+.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::List(Box::new(Ty::Int)))));
    }

    #[test]
    fn constraint_collection_does_not_carry_a_heterogeneous_list_literal() {
        // D-146 (#239): a heterogeneous `int`/`float` list keeps the
        // historical `Ok(None)` behavior -- exact `Ty` equality (not
        // `merge_inferred_types`) determines homogeneity, so `int` and
        // `float` are not merged even though `merge_inferred_types` would
        // reject them anyway.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::FloatLiteral(2.0),
        ]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_does_not_carry_a_bool_int_heterogeneous_list_literal() {
        // D-146 (#239): a heterogeneous `int`/`bool` list keeps the
        // historical `Ok(None)` behavior -- exact `Ty` equality (not
        // `merge_inferred_types`) determines homogeneity, so `bool` and
        // `int` are not merged even though `merge_inferred_types` would
        // widen `bool` to `int`.  This mirrors
        // `infer_expr_in`'s own list homogeneity rule (D-105).
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::BoolLiteral(true),
        ]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_does_not_carry_an_empty_list_literal() {
        // D-146 (#239): an empty list has no element type to carry -- keeps
        // the historical `Ok(None)` behavior.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_does_not_carry_a_list_literal_with_a_non_scalar_element() {
        // D-146 (#239): a nested-container element (`list[list[int]]`) is
        // rejected by the `is_private_solver_scalar` gate -- the carrier is
        // scalar-element-only to prevent nested-container carriers this
        // solver has no representation for.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
        ])]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_does_not_carry_a_list_literal_when_an_element_has_no_term() {
        // D-146 (#239): when an element produces `None` (here an unbound
        // module-global name, not a local so no `T0021`), the list keeps the
        // historical `Ok(None)` behavior -- the carrier requires every
        // element to produce `Some(Ok(ty))`.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::Name("unbound_global".to_string()),
        ]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_subscript_on_a_list_literal_base_extracts_the_element_type() {
        // D-146 (#239): `xs[0]` where `xs` is a `Ty::List`-bound name
        // extracts the scalar element type. This is the core reproduction
        // for #239: before this fix the `Subscript` arm returned `Ok(None)`
        // regardless of the base's resolved type.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("xs".to_string(), Ok(Ty::List(Box::new(Ty::Int))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("xs".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_subscript_on_a_non_list_bound_base_keeps_ok_none() {
        // D-146 (#239): a `Ty::Int`-bound name is not a `Ty::List` carrier,
        // so the `Subscript` arm keeps the historical `Ok(None)` behavior --
        // the real check pass (`infer_expr_in`) validates it later.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("x".to_string(), Ok(Ty::Int))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_subscript_on_an_unresolved_list_base_keeps_ok_none() {
        // D-146 (#239): a genuinely unresolved inference variable (a fresh
        // term, not yet unified) is not a `Ty::List` carrier, so the
        // `Subscript` arm keeps the historical `Ok(None)` behavior -- the
        // real check pass validates it once its type is actually known.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let unresolved = fresh_term(&mut parents, &mut concrete);
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("xs".to_string(), unresolved)]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("xs".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_list_pop_on_a_list_typed_bound_name_extracts_the_element_type() {
        // D-146 (#239): `xs.pop()` where `xs` is a `Ty::List`-bound name
        // extracts the scalar element type -- the carrier is destructured,
        // never unified.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("xs".to_string(), Ok(Ty::List(Box::new(Ty::Str))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListPop {
            list: "xs".to_string(),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Str)));
    }

    #[test]
    fn constraint_collection_list_pop_on_an_unbound_name_keeps_ok_none() {
        // D-146 (#239): `xs.pop()` where `xs` is not in `env.bindings` keeps
        // the historical `Ok(None)` behavior -- the real check pass
        // (`infer_expr_in`) validates it later.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListPop {
            list: "xs".to_string(),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_list_pop_on_a_non_list_bound_name_keeps_ok_none() {
        // D-146 (#239): `xs.pop()` where `xs` is a `Ty::Int`-bound name is
        // not a `Ty::List` carrier, so the `ListPop` arm keeps the
        // historical `Ok(None)` behavior.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("xs".to_string(), Ok(Ty::Int))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListPop {
            list: "xs".to_string(),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_list_append_after_a_list_binding_still_keeps_ok_none() {
        // D-146 (#239): `ListAppend` is unchanged -- it still returns
        // `Ok(None)` and recurses into `value` only, even when the list is
        // bound. The carrier is for element-type extraction, not for
        // append's own void (`Ty::None`) result.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("xs".to_string(), Ok(Ty::List(Box::new(Ty::Int))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListAppend {
            list: "xs".to_string(),
            value: Box::new(HirExpr::IntLiteral(2)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_inline_subscript_on_a_list_literal_extracts_the_element_type() {
        // D-146 (#239): `[1][0]` -- an inline subscript on a list literal --
        // extracts the element type. The `ListLiteral` arm produces the
        // carrier, and the `Subscript` arm destructures it.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_an_empty_slice() {
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&[]),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_a_none_element_term() {
        let terms = vec![Some(Ok(Ty::Int)), None];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_a_none_first_element() {
        // Covers the `.as_ref()?` None path when the first element itself is
        // `None` (distinct from the test above where `None` is at index 1).
        let terms = vec![None, Some(Ok(Ty::Int))];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_an_err_element_term() {
        let terms = vec![Some(Ok(Ty::Int)), Some(Err(0))];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_an_err_first_element() {
        // Covers the `.ok()?` None path when the first element is `Some(Err(_))`
        // (distinct from the test above where `Err` is at index 1).
        let terms = vec![Some(Err(0)), Some(Ok(Ty::Int))];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_a_non_scalar_first_element() {
        let terms = vec![Some(Ok(Ty::List(Box::new(Ty::Int))))];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_rejects_heterogeneous_scalars() {
        let terms = vec![Some(Ok(Ty::Int)), Some(Ok(Ty::Bool))];
        // Exact `Ty` equality, NOT `merge_inferred_types` -- `bool` and `int`
        // are not merged even though `merge_inferred_types` would widen them.
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            None
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_accepts_homogeneous_scalars() {
        let terms = vec![Some(Ok(Ty::Int)), Some(Ok(Ty::Int))];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            Some(Ty::Int)
        );
    }

    #[test]
    fn homogeneous_private_solver_scalar_list_element_accepts_a_single_scalar() {
        let terms = vec![Some(Ok(Ty::Str))];
        assert_eq!(
            homogeneous_private_solver_scalar_list_element(&terms),
            Some(Ty::Str)
        );
    }

    #[test]
    fn constraint_collection_treats_a_slice_as_unconstrained_but_recurses_into_base_and_bounds() {
        // PR-12 Task 7 (D-118): mirrors `Subscript`'s own solver arm above --
        // structurally identical recursion, no term produced for the slice
        // itself.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: Some(Box::new(HirExpr::IntLiteral(0))),
            stop: Some(Box::new(HirExpr::IntLiteral(2))),
            step: Some(Box::new(HirExpr::IntLiteral(1))),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_treats_a_slice_with_every_bound_omitted_as_unconstrained() {
        // Proves the `Option` loop's `None` branch is exercised too, not
        // just the `Some` branch above (`xs[:]`).
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: None,
            stop: None,
            step: None,
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_slice_base() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("missing".to_string())),
            start: None,
            stop: None,
            step: None,
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_slice_bound() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: Some(Box::new(HirExpr::Name("missing".to_string()))),
            stop: None,
            step: None,
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_list_append_as_unconstrained_but_recurses_into_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListAppend {
            list: "lst".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_list_append_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListAppend {
            list: "lst".to_string(),
            value: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    // -- PR-12 Task 10 (D-119): remaining container methods depth --------
    // `collect_expr_constraints` coverage, mirroring `ListAppend`'s own
    // direct-call test shape exactly.

    #[test]
    fn constraint_collection_treats_a_list_pop_as_unconstrained() {
        // `list` is a plain name, not a sub-expression -- unlike
        // `ListAppend`, there is no `value` to recurse into here.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::ListPop {
            list: "lst".to_string(),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_treats_a_dict_get_or_default_as_unconstrained_but_recurses_into_key_and_default()
     {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_get_or_default_key() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::Name("missing".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_get_or_default_default() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_set_add_as_unconstrained_but_recurses_into_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_set_add_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_len_call_returns_int_for_a_concretely_bound_list() {
        // `lst`'s binding is a directly concrete `TypeTerm` (`Ok(Ty::List(_))`)
        // rather than one produced by `ListLiteral` (which always returns
        // `Ok(None)` in this solver, per its own comment above) -- this is
        // the only way to get a concrete `Ty::List` term to validate against
        // at constraint-collection time.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("lst".to_string(), Ok(Ty::List(Box::new(Ty::Int))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("lst".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_len_call_defers_an_unresolved_argument_to_the_real_check_pass() {
        // `lst`'s binding is a genuinely unresolved inference variable (a
        // fresh term, not yet unified with anything) -- this solver can't
        // tell yet whether it's a list, so it must not reject it here; the
        // real check pass (`infer_expr_in`) validates it once its type is
        // actually known.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let unresolved = fresh_term(&mut parents, &mut concrete);
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("lst".to_string(), unresolved)]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("lst".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_len_call_rejects_the_wrong_arity() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![],
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`len` expects exactly 1 argument, got 0");
    }

    #[test]
    fn constraint_collection_len_call_rejects_a_concretely_known_non_list_argument() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::IntLiteral(5)],
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `int`"
        );
    }

    #[test]
    fn constraint_collection_float_call_returns_float_regardless_of_argument_resolution() {
        // Mirrors `constraint_collection_len_call_returns_int_for_a_
        // concretely_bound_list`: a directly concrete `TypeTerm`
        // (`Ok(Ty::Int)`) validates cleanly and produces `Ty::Float`.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("x".to_string(), Ok(Ty::Int))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Float)));
    }

    #[test]
    fn constraint_collection_float_call_defers_an_unresolved_argument_to_the_real_check_pass() {
        // Mirrors `constraint_collection_len_call_defers_an_unresolved_
        // argument_to_the_real_check_pass`: a genuinely unresolved
        // inference variable is left to `infer_expr_in`, but the call's
        // own return type (`Ty::Float`) is still produced unconditionally.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let unresolved = fresh_term(&mut parents, &mut concrete);
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("x".to_string(), unresolved)]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Float)));
    }

    #[test]
    fn constraint_collection_float_call_rejects_the_wrong_arity() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![],
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`float` expects exactly 1 argument, got 0");
    }

    #[test]
    fn constraint_collection_float_call_rejects_a_concretely_known_non_numeric_argument() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::StringLiteral("hello".to_string())],
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "`float` expects an `int`, `float`, or `bool` argument, got `str`"
        );
    }

    #[test]
    fn constraint_collection_honors_a_user_defined_float_signature_over_the_builtin() {
        // Same post-merge review finding as `infer_expr_in`'s own
        // `a_user_defined_float_function_takes_priority_over_the_builtin`:
        // a registered `float` signature (e.g. one accepting `str`, which
        // the builtin itself would reject) must resolve through the normal
        // signature lookup, not the hand-recognized builtin arm.
        let signatures = HashMap::from([(
            "float".to_string(),
            (vec!["x".to_string()], vec![Ok(Ty::Str)], Ok(Ty::Str)),
        )]);
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::StringLiteral("hello".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Str)));
    }

    #[test]
    fn collect_block_constraints_gives_a_for_list_loop_variable_a_fresh_term_when_unbound() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["i"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "lst".to_string(),
            body: vec![],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.bindings.contains_key("i"));
    }

    #[test]
    fn collect_block_constraints_keeps_a_for_list_loop_variable_s_existing_term() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("i".to_string(), Ok(Ty::Int))]),
            local_names: &["i"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "lst".to_string(),
            body: vec![],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert_eq!(env.bindings.get("i"), Some(&Ok(Ty::Int)));
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_for_list_loop_body() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["i", "z"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "lst".to_string(),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("z".to_string()))],
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_private_helper_can_use_an_annotated_assignment_during_signature_inference() {
        // End-to-end: an annotated assignment inside a private helper whose
        // own signature is still `Ty::Infer` exercises the solver path
        // (`collect_block_constraints`) as well as the later concrete
        // re-check (`check_stmt_in_function`), not just the direct unit
        // tests above.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::AnnAssign {
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::IntLiteral(1)),
                }],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn an_annotated_private_local_preserves_known_bool_initializer_evidence() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::Int,
                            value: Some(HirExpr::Name("x".to_string())),
                        },
                        HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::BoolLiteral(true)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        check(&hir).unwrap();
        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[0],
            HirItem::Function { params, return_ty, .. }
                if params[0].1 == Ty::Bool && *return_ty == Ty::Int
        ));
    }

    #[test]
    fn an_annotated_private_local_does_not_retag_the_returned_initializer() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_echo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::Int,
                            value: Some(HirExpr::Name("x".to_string())),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_echo".to_string(),
                    args: vec![HirExpr::BoolLiteral(true)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[0],
            HirItem::Function { params, return_ty, .. }
                if params[0].1 == Ty::Bool && *return_ty == Ty::Bool
        ));
    }

    #[test]
    fn an_annotated_private_local_supplies_a_fallback_without_call_site_evidence() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("x".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::AnnAssign {
                        target: "y".to_string(),
                        annotation: Ty::Int,
                        value: Some(HirExpr::Name("x".to_string())),
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[0],
            HirItem::Function { params, return_ty, .. }
                if params[0].1 == Ty::Int && *return_ty == Ty::Int
        ));
    }

    #[test]
    fn a_container_annotation_cannot_escape_the_scalar_only_private_solver() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("x".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::AnnAssign {
                        target: "y".to_string(),
                        annotation: Ty::List(Box::new(Ty::Int)),
                        value: Some(HirExpr::Name("x".to_string())),
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot infer type of parameter `x`"));
    }

    #[test]
    fn a_container_annotated_target_cannot_resolve_an_inferred_private_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::AnnAssign {
                        target: "y".to_string(),
                        annotation: Ty::List(Box::new(Ty::Int)),
                        value: Some(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message
                .contains("cannot infer return type of private helper `_helper`")
        );
    }

    #[test]
    fn hard_call_evidence_cannot_turn_a_container_local_into_an_inferred_signature() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "_helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::List(Box::new(Ty::Int)),
                            value: Some(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
                        },
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_sink".to_string(),
                            args: vec![HirExpr::Name("y".to_string())],
                        }),
                        HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message
                .contains("cannot infer return type of private helper `_helper`")
        );
    }

    #[test]
    fn an_unrelated_container_local_does_not_block_scalar_signature_inference() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_helper".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::List(Box::new(Ty::Int)),
                            value: Some(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_helper".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[0],
            HirItem::Function { params, return_ty, .. }
                if params[0].1 == Ty::Int && *return_ty == Ty::Int
        ));
    }

    #[test]
    fn scalar_hard_evidence_on_a_container_local_reaches_final_validation() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "_helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::List(Box::new(Ty::Int)),
                            value: Some(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
                        },
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_sink".to_string(),
                            args: vec![HirExpr::Name("y".to_string())],
                        }),
                        HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message
                .contains("argument 1 of `_sink` expects `int`, got `list[int]`")
        );
    }

    #[test]
    fn an_annotated_private_local_fallback_propagates_through_a_binary_expression() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("x".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::AnnAssign {
                        target: "y".to_string(),
                        annotation: Ty::Int,
                        value: Some(HirExpr::BinOp {
                            op: BinOpKind::Add,
                            left: Box::new(HirExpr::Name("x".to_string())),
                            right: Box::new(HirExpr::IntLiteral(1)),
                        }),
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[0],
            HirItem::Function { params, .. } if params[0].1 == Ty::Int
        ));
    }

    #[test]
    fn an_annotated_private_local_fallback_rechecks_binary_operand_conflicts() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("x".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::AnnAssign {
                        target: "y".to_string(),
                        annotation: Ty::Int,
                        value: Some(HirExpr::BinOp {
                            op: BinOpKind::Add,
                            left: Box::new(HirExpr::Name("x".to_string())),
                            right: Box::new(HirExpr::StringLiteral("bad".to_string())),
                        }),
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message
                .contains("right operand of int binary expression")
        );
    }

    #[test]
    fn later_nested_call_evidence_wins_over_an_earlier_annotation_fallback() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_echo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::Int,
                            value: Some(HirExpr::Name("x".to_string())),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
                HirItem::Function {
                    name: "_caller".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_echo".to_string(),
                            args: vec![HirExpr::BoolLiteral(true)],
                        }),
                        HirStmt::Return(None),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[0],
            HirItem::Function { params, return_ty, .. }
                if params[0].1 == Ty::Bool && *return_ty == Ty::Bool
        ));
    }

    #[test]
    fn multiple_annotation_bounds_choose_bool_in_either_declaration_order() {
        for bool_first in [false, true] {
            let int_assignment = HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Name("x".to_string())),
            };
            let bool_assignment = HirStmt::AnnAssign {
                target: "b".to_string(),
                annotation: Ty::Bool,
                value: Some(HirExpr::Name("x".to_string())),
            };
            let assignments = if bool_first {
                vec![bool_assignment, int_assignment]
            } else {
                vec![int_assignment, bool_assignment]
            };
            let mut body = assignments;
            body.push(HirStmt::Return(Some(HirExpr::Name("b".to_string()))));
            let hir = HirModule {
                items: vec![HirItem::Function {
                    name: "_helper".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body,
                }],
                type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
            };

            let resolved = check_and_resolve(&hir).unwrap();
            assert!(matches!(
                &resolved.items[0],
                HirItem::Function { params, .. } if params[0].1 == Ty::Bool
            ));
        }
    }

    #[test]
    fn incompatible_annotation_bounds_fail_identically_in_either_declaration_order() {
        let mut messages = Vec::new();
        for string_first in [false, true] {
            let int_assignment = HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Name("x".to_string())),
            };
            let string_assignment = HirStmt::AnnAssign {
                target: "b".to_string(),
                annotation: Ty::Str,
                value: Some(HirExpr::Name("x".to_string())),
            };
            let body = if string_first {
                vec![string_assignment, int_assignment]
            } else {
                vec![int_assignment, string_assignment]
            };
            let hir = HirModule {
                items: vec![HirItem::Function {
                    name: "_helper".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body,
                }],
                type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
            };

            let err = check_and_resolve(&hir).unwrap_err();
            assert_eq!(err.code, "T0021");
            messages.push(err.message);
        }
        assert_eq!(messages[0], messages[1]);
    }

    #[test]
    fn annotated_private_local_rejects_known_int_to_bool_narrowing_with_t0025() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_narrow".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::Bool,
                            value: Some(HirExpr::Name("x".to_string())),
                        },
                        HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_narrow".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0025");
    }

    #[test]
    fn annotated_private_local_binds_when_a_container_subscript_has_no_solver_term() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::Function {
                    name: "_first".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::Int,
                            value: Some(HirExpr::Subscript {
                                base: Box::new(HirExpr::Name("xs".to_string())),
                                index: Box::new(HirExpr::IntLiteral(0)),
                            }),
                        },
                        HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert!(matches!(
            &resolved.items[1],
            HirItem::Function { return_ty, .. } if *return_ty == Ty::Int
        ));
    }

    #[test]
    fn no_term_initializer_still_reaches_directional_final_validation() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::Function {
                    name: "_first".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "y".to_string(),
                            annotation: Ty::Bool,
                            value: Some(HirExpr::Subscript {
                                base: Box::new(HirExpr::Name("xs".to_string())),
                                index: Box::new(HirExpr::IntLiteral(0)),
                            }),
                        },
                        HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0025");
    }

    #[test]
    fn constraint_collection_leaves_top_level_return_to_validation() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Return(Some(HirExpr::IntLiteral(1)))),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert_eq!(check(&hir).unwrap_err().code, "T0024");
    }

    #[test]
    fn a_cloned_environment_keeps_later_function_bindings_isolated() {
        let mut original = Environment::new();
        original.bind_function("original".to_string(), vec![Ty::Int], Ty::Int);

        let mut cloned = original.clone();
        cloned.bind_function("cloned".to_string(), vec![], Ty::None);

        assert!(original.lookup_function("cloned").is_none());
        assert_eq!(
            cloned.lookup_function("cloned"),
            Some(&(Vec::new(), Ty::None))
        );
        assert_eq!(
            original.lookup_function("original"),
            Some(&(vec![Ty::Int], Ty::Int))
        );
    }

    #[test]
    fn infers_an_int_literal_as_int() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::IntLiteral(1)), Ok(Ty::Int));
    }

    #[test]
    fn infers_a_float_literal_as_float() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::FloatLiteral(1.5)), Ok(Ty::Float));
    }

    #[test]
    fn infers_a_bool_literal_as_bool() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::BoolLiteral(true)), Ok(Ty::Bool));
    }

    #[test]
    fn infers_a_string_literal_as_str() {
        let env = Environment::new();
        assert_eq!(
            infer_expr(&env, &HirExpr::StringLiteral("hi".to_string())),
            Ok(Ty::Str)
        );
    }

    #[test]
    fn adding_an_int_and_a_str_is_a_clean_type_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn adding_two_strings_infers_str() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::StringLiteral("b".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn subtracting_two_strings_is_a_clean_type_error() {
        // Python allows `"a" + "b"` but no other arithmetic operator between
        // two strings -- `"a" - "b"` is a `TypeError` at runtime in CPython.
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Sub,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::StringLiteral("b".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_two_strings_infers_bool() {
        // `"a" == "b"`, `"a" < "b"`, etc. are ordinary, valid Python
        // (lexicographic ordering) -- not covered by numeric_or_bool_compatible
        // before `Ty::Str` became constructible via literals.
        let env = Environment::new();
        for op in [
            CmpOpKind::Eq,
            CmpOpKind::NotEq,
            CmpOpKind::Lt,
            CmpOpKind::LtE,
            CmpOpKind::Gt,
            CmpOpKind::GtE,
        ] {
            let expr = HirExpr::Compare {
                op,
                left: Box::new(HirExpr::StringLiteral("a".to_string())),
                right: Box::new(HirExpr::StringLiteral("b".to_string())),
            };
            assert_eq!(
                infer_expr(&env, &expr),
                Ok(Ty::Bool),
                "comparison {op:?} should type-check"
            );
        }
    }

    #[test]
    fn an_f_string_always_infers_str_regardless_of_interpolated_types() {
        let env = Environment::new();
        let expr = HirExpr::FString(vec![
            FStringPart::Literal("n=".to_string()),
            FStringPart::Interpolation(Box::new(HirExpr::IntLiteral(1))),
        ]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn an_f_string_still_type_checks_its_interpolated_expressions() {
        let env = Environment::new();
        let expr = HirExpr::FString(vec![FStringPart::Interpolation(Box::new(HirExpr::Name(
            "undefined".to_string(),
        )))]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_a_string_and_an_int_is_a_clean_type_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_two_ints_infers_bool() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Lt,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn comparing_a_bool_and_an_int_succeeds_since_bool_is_a_subtype_of_int() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn comparing_an_undefined_left_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::Name("undefined".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_an_undefined_right_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Name("undefined".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_incompatible_types_is_a_clean_type_error() {
        let mut env = Environment::new();
        // A call to a properly declared, zero-arg, `None`-returning function
        // legitimately infers `Ty::None`, which isn't numeric-like --
        // comparing an int against it is a genuine, both-sides-defined
        // incompatibility.
        env.bind_function("f".to_string(), vec![], Ty::None);
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Call {
                callee: "f".to_string(),
                args: vec![],
            }),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("int") && err.message.contains("None"));
    }

    #[test]
    fn a_binop_treats_bool_as_int() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::BoolLiteral(true)),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_binop_treats_bool_and_float_as_float() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::BoolLiteral(true)),
            right: Box::new(HirExpr::FloatLiteral(1.5)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn a_top_level_return_is_a_clean_diagnostic_not_a_panic() {
        // Regression test (self-review finding, pre-merge): this used to be
        // `panic!(...)`, so a bare `return` at module scope crashed the
        // compiler (exit code 101) instead of producing a diagnostic through
        // the documented exit-1 contract every other error path uses.
        // `ruff_python_parser` does not reject `return` outside a function at
        // the grammar level (CPython itself only rejects it in a later
        // compile pass), so this is reachable from ordinary CLI input.
        let mut env = Environment::new();
        let err = check_stmt(&mut env, &HirStmt::Return(None)).unwrap_err();
        assert_eq!(err.code, "T0024");
    }

    #[test]
    fn check_and_resolve_also_rejects_a_top_level_return_with_t0024() {
        // Regression test: `collect_block_constraints` (the private-helper
        // solver) is invoked over top-level statements with `return_term:
        // None` (no enclosing function), so a top-level `Return` hits its
        // own defensive `let Some(return_term) = return_term else {
        // continue }` arm and is silently skipped by the solver -- it's
        // `check_and_resolve`'s later, ordinary `check_stmt` pass (Pass 2)
        // that actually rejects it with T0024, exactly like the
        // `pycc_types::check` entry point already does.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Return(None))],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0024");
    }

    #[test]
    fn a_return_nested_in_a_top_level_if_is_also_a_clean_diagnostic() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Return(None)],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0024");
    }

    #[test]
    fn an_assignment_binds_the_inferred_type_in_the_environment() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_assignment_whose_value_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Name("undefined".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_incompatible_reassignment_is_t0023_and_preserves_the_inferred_type() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("changed".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_annotated_assignment_with_a_matching_value_binds_the_annotation_type() {
        // x: int = True -- bool is assignable to int (matches is_assignable's
        // existing widening rule), and the environment should record Ty::Int
        // (the annotation), not Ty::Bool (the initializer's own inferred type).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::BoolLiteral(true)),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_annotated_assignment_with_a_mismatched_value_is_t0025() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::StringLiteral("nope".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0025");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_annotated_assignment_propagates_an_error_from_the_initializer_expression() {
        // The initializer itself can fail to type-check (here, referencing an
        // undefined name) before `is_assignable` is ever consulted -- the `?`
        // on `infer_expr` inside the new arm must propagate that error rather
        // than being reachable only via the is_assignable mismatch path.
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Name("undefined".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_annotation_only_declaration_does_not_bind_a_value_at_module_scope() {
        // x: int alone must only declare x, not bind it -- collect_local_names
        // registers `x` as local independently of this arm, so a premature
        // read still falls through to the existing T0021 mechanism (see the
        // function-scope regression test below for the end-to-end proof).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn value_less_declaration_then_matching_assignment_binds_the_declared_type() {
        // x: int; x = 1 -- issue #245: the value-less declaration must be
        // retained and honored by the later plain assignment.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn value_less_declaration_then_mismatched_assignment_is_t0026() {
        // x: int; x = "hello" -- issue #245's primary reproduction: this must
        // be rejected, and rejected with T0026 (not T0023 -- nothing was
        // "previously inferred", it was declared, never assigned).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0026");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn value_less_declaration_still_raises_t0021_on_a_premature_read_at_function_scope() {
        // x: int; return x (no assignment in between) -- the declared type
        // must never satisfy `lookup`, so this stays T0021, exactly as an
        // ordinary local without any annotation would.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "x".to_string(),
                            annotation: Ty::Int,
                            value: None,
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                    }],
                })),
            ],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn repeated_compatible_value_less_declaration_then_assignment_is_accepted() {
        // x: int; x: int; x = 1 -- a second declaration compatible with the
        // first is accepted (first-declaration-wins), and the later plain
        // assignment still binds the declared type.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn repeated_incompatible_value_less_declaration_is_t0026() {
        // x: int; x: str -- flatly incompatible re-declarations are rejected
        // at the second declaration, before any assignment is even seen.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: None,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0026");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn valued_redeclaration_matching_an_earlier_value_less_declaration_keeps_the_declared_type() {
        // x: int; x: bool = True -- the *earlier* declaration wins as the
        // sticky representation (Int), since bool is merely assignable to
        // int, not the later annotation (matches check_assignment's own
        // worked bool/int example in its declared-consult branch).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Bool,
                value: Some(HirExpr::BoolLiteral(true)),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn valued_redeclaration_disagreeing_with_an_earlier_value_less_declaration_is_t0026() {
        // x: int; x: str = "hello" -- T0025 passes (the initializer matches
        // its own `str` annotation), so this must be rejected by the
        // declared-consult step with T0026, not silently accepted.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: Some(HirExpr::StringLiteral("hello".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0026");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn value_less_declaration_after_an_existing_binding_is_a_no_op() {
        // x = 1; x: int -- re-declaring an already-bound name is unchanged,
        // pre-existing, out-of-scope behavior: `declare` must not shadow the
        // real binding with an inert `declared` entry.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn function_scope_value_less_declaration_then_matching_assignment_binds_the_declared_type() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
            Ty::None,
        )
        .unwrap();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn function_scope_value_less_declaration_then_mismatched_assignment_is_t0026() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
            Ty::None,
        )
        .unwrap();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0026");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn function_scope_repeated_incompatible_value_less_declaration_is_t0026() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
            Ty::None,
        )
        .unwrap();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: None,
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0026");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn child_for_function_clears_a_declared_entry_for_a_shadowed_local_name() {
        // A module-level value-less declaration for `x` must not leak into
        // an unrelated function's own local `x` -- mirrors the existing
        // `bindings`-clearing behavior `child_for_function` already provides.
        let mut module_env = Environment::new();
        check_stmt(
            &mut module_env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        let mut fn_env = module_env.child_for_function(&["x"]);
        // The function's own body assigns a `str` to its local `x` -- if the
        // module-level `declared` entry leaked through, this would wrongly
        // fail with T0026 against the stale `Int` declaration.
        check_stmt_in_function(
            &mut fn_env,
            &["x"],
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            },
            Ty::None,
        )
        .unwrap();
        assert_eq!(fn_env.lookup("x"), Some(Ty::Str));
    }

    #[test]
    fn function_scope_annotated_assignment_with_a_matching_value_binds_the_annotation_type() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::BoolLiteral(true)),
            },
            Ty::None,
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn function_scope_annotated_assignment_with_a_mismatched_value_is_t0025() {
        let mut env = Environment::new();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::StringLiteral("nope".to_string())),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0025");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn function_scope_annotated_assignment_propagates_an_error_from_the_initializer_expression() {
        // Same as the module-scope sibling above, but through
        // `infer_expr_in`'s local-vs-global unbound distinction: `x` is a
        // declared local that was never assigned, so referencing it in `y`'s
        // initializer must propagate T0021 via the new arm's `?` rather than
        // silently falling through to the is_assignable check.
        let mut env = Environment::new();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "y".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Name("x".to_string())),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("y"), None);
    }

    #[test]
    fn function_scope_annotation_only_declaration_does_not_bind_a_value() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
            Ty::None,
        )
        .unwrap();
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_annotation_only_declaration_does_not_bind_a_value() {
        // x: int alone must not make a later use of x succeed -- it only
        // declares x local, it does not bind it (matching CPython's own
        // UnboundLocalError for this exact shape, verified during planning;
        // this end-to-end module mirrors
        // tests/diagnostics/d0026_annotation_only_unbound.py exactly).
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "x".to_string(),
                            annotation: Ty::Int,
                            value: None,
                        },
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![HirExpr::Name("x".to_string())],
                        }),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        // Pin the exact message, not just the shared "T0021" code: this test's
        // entire purpose is proving the value-less form reaches the
        // *local-unbound* T0021 path (`unbound_local`), not the differently
        // worded *undefined-name* T0021 path -- both share the same code, so
        // only the message distinguishes them.
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn an_annotated_re_declaration_that_conflicts_with_an_existing_binding_is_t0023() {
        // `x`'s representation is fixed by its first assignment (Int) --
        // `pycc_mir`'s own `bind_variable` keeps that first type regardless of
        // a later compatible reassignment, so the checker must reject a later
        // annotated re-declaration that would otherwise silently repoint `x`
        // at an incompatible representation (here, `str`) via `is_assignable`
        // trivially comparing the initializer to its own annotation alone.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: Some(HirExpr::StringLiteral("s".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn function_scope_annotated_re_declaration_that_conflicts_with_an_existing_binding_is_t0023() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: Some(HirExpr::StringLiteral("s".to_string())),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn assigning_bool_to_an_int_binding_keeps_the_declared_representation() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BoolLiteral(true),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_for_target_cannot_change_an_existing_binding_representation() {
        let mut env = Environment::new();
        env.bind("value".to_string(), Ty::Str);
        let err = check_stmt(
            &mut env,
            &HirStmt::ForRange {
                var: "value".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("value"), Some(Ty::Str));
    }

    #[test]
    fn a_for_target_cannot_change_a_parameter_representation() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "loop_over".to_string(),
                params: vec![("value".to_string(), Ty::Str)],
                return_ty: Ty::None,
                body: vec![HirStmt::ForRange {
                    var: "value".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn direct_function_check_rejects_a_for_target_representation_change() {
        let function = HirItem::Function {
            name: "loop_over".to_string(),
            params: vec![("value".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "value".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_private_for_target_infers_an_unannotated_parameter_as_int() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_loop".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::None,
                body: vec![HirStmt::ForRange {
                    var: "value".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_loop".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::None,
                body: vec![HirStmt::ForRange {
                    var: "value".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }],
            }
        );
    }

    #[test]
    fn an_if_s_test_must_be_bool_like_and_both_branches_are_checked() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::IntLiteral(2),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: each branch is checked in an independent clone
        // of env, then joined. `x` is bound only in the body, `y` only in
        // orelse -- both are `Maybe` after the join, so `lookup` (which
        // returns `Some` only for `Definitely`) returns `None` for both.
        // Use `lookup_any` to verify the types are retained.
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup("y"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
        assert_eq!(env.lookup_any("y"), Some(Ty::Int));
    }

    #[test]
    fn an_if_with_both_branches_binding_the_same_name_joins_to_definitely() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(2),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Both branches bind `x` as `int` -> join is `Definitely`.
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_no_else_if_makes_body_bindings_maybe() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // No else -> body-only binding is `Maybe`.
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn a_nested_no_else_if_makes_bindings_maybe() {
        // `if a: if b: x = 1` -- x is Maybe after the outer if (no else on
        // either if).
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                orelse: vec![],
            }],
            orelse: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_if_else_with_nested_if_in_orelse_joins_to_maybe() {
        // `if a: x = 1 else: if b: x = 2` -- x is Maybe (orelse's if has no
        // else, so x is maybe in orelse; join of definite and maybe = maybe).
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
                orelse: vec![],
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_if_with_maybe_in_body_and_definite_in_orelse_joins_to_maybe() {
        // `if a: if b: x = 1 else: x = 2` -- x is Maybe (body's inner if has
        // no else, so x is maybe in body; join of maybe and definite = maybe).
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                orelse: vec![],
            }],
            orelse: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(2),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_if_with_maybe_in_both_branches_joins_to_maybe() {
        // `if a: if b: x = 1 else: if c: x = 2` -- x is Maybe in both
        // branches (each branch's inner if has no else, so x is Maybe in
        // each). The join of Maybe and Maybe is Maybe. Exercises the
        // (BindingState::Maybe(_), Some(BindingState::Maybe(_))) arm.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                orelse: vec![],
            }],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
                orelse: vec![],
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_if_with_maybe_in_orelse_only_joins_to_maybe() {
        // `if a: pass else: if b: x = 1` -- x is Maybe (orelse's inner if
        // has no else, so x is maybe in orelse; body doesn't bind x at all).
        // This exercises join_if_branches' pass 2 with a Maybe binding.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                orelse: vec![],
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_if_with_a_while_inside_that_assigns_takes_the_clone_path() {
        // `if a: while b: x = 1` -- the while inside the if body introduces
        // bindings, so the fast path is NOT taken and the clone+join path
        // runs. Exercises introduces_bindings' While arm.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
            }],
            orelse: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_if_with_a_for_list_inside_that_assigns_takes_the_clone_path() {
        // `if a: for x in items: y = 1` -- the for-list inside the if body
        // introduces bindings, so the fast path is NOT taken. Exercises
        // introduces_bindings' ForList arm.
        let mut env = Environment::new();
        env.bind("items".to_string(), Ty::List(Box::new(Ty::Int)));
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ForList {
                var: "x".to_string(),
                list: "items".to_string(),
                body: vec![HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
            }],
            orelse: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
    }

    #[test]
    fn an_elif_chain_without_else_joins_to_maybe() {
        // `if a: x = 1 elif b: x = 2` (lowered to nested If in orelse) --
        // x is Maybe (inner if has no else).
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
                orelse: vec![],
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn an_elif_chain_with_else_joins_to_definitely() {
        // `if a: x = 1 elif b: x = 2 else: x = 3` -- all paths bind x.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
                orelse: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(3),
                }],
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_pre_bound_name_in_if_stays_definitely() {
        // `x = 1; if cond: x = 2` -- x was definite before, stays definite.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
                orelse: vec![],
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_type_mismatch_at_if_join_is_t0023() {
        // `if cond: x = 1 else: x = "hello"` -- incompatible types at join.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0023");
    }

    #[test]
    fn a_maybe_bound_callee_is_t0041() {
        // `if cond: f = 1` then `f()` -- f is maybe-bound, not callable.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "f".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                orelse: vec![],
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::ExprStmt(HirExpr::Call {
                callee: "f".to_string(),
                args: vec![],
            }),
        )
        .unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn an_if_whose_test_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::Name("undefined".to_string()),
            body: vec![],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_whose_orelse_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_with_assignments_whose_body_has_an_error_propagates_via_clone_path() {
        // Body has an assignment (so fast path is NOT taken) AND an error.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![
                HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::ExprStmt(HirExpr::Name("undefined".to_string())),
            ],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_with_assignments_whose_orelse_has_an_error_propagates_via_clone_path() {
        // Body has an assignment (so fast path is NOT taken), orelse has error.
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_with_assignments_whose_body_has_an_error_propagates_via_clone_path() {
        // Body has an assignment (so fast path is NOT taken) AND an error.
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![
                HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::ExprStmt(HirExpr::Name("undefined".to_string())),
            ],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_loop_s_test_and_body_are_checked() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: the loop body may execute zero times, so
        // body-only binding `x` is `Maybe` after the loop.
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn a_while_loop_body_then_read_is_t0041() {
        // `while True: x = 1` then `x` read -- T0041 (the loop body may
        // execute zero times, so `x` is maybe-bound).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
        )
        .unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_pre_bound_variable_survives_a_while_loop_as_definite() {
        // `x = 1; while True: x = 2` -- `x` was definitely bound before the
        // loop, so it stays `Definitely` after the loop.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                }],
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_while_loop_with_nested_if_body_joins_to_maybe() {
        // `while True: if True: x = 1` -- x is Maybe after the loop (the
        // inner if has no else, so x is Maybe in the body; join_loop_body
        // downgrades it to Maybe). Exercises the Maybe alternative in
        // join_loop_body's inner match.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(1),
                    }],
                    orelse: vec![],
                }],
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn a_for_range_var_read_after_loop_is_t0041() {
        // `for i in range(3): pass` then `i` read -- T0041 (the loop variable
        // is maybe-bound after the loop).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::ExprStmt(HirExpr::Name("i".to_string())),
        )
        .unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_pre_bound_for_range_var_survives_as_definite() {
        // `i = 0; for i in range(3): pass` then `i` read -- succeeds because
        // `i` was definitely bound before the loop.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "i".to_string(),
                value: HirExpr::IntLiteral(0),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        )
        .unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
    }

    #[test]
    fn a_while_loop_whose_test_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::Name("undefined".to_string()),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_binds_its_variable_as_int_and_checks_its_body() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Name("i".to_string()),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: the loop may execute zero times, so both the
        // loop variable `i` and body-only binding `x` are `Maybe`.
        assert_eq!(env.lookup("i"), None);
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("i"), Some(Ty::Int));
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn a_pre_bound_name_survives_a_for_range_loop_as_definitely() {
        // `x = 1; for i in range(3): pass` -- x stays Definitely, i is Maybe.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
        assert_eq!(env.lookup("i"), None);
        assert_eq!(env.lookup_any("i"), Some(Ty::Int));
    }

    #[test]
    fn a_for_range_loop_whose_start_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::Name("undefined".to_string()),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_stop_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::Name("undefined".to_string()),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_step_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::Name("undefined".to_string()),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_rejects_a_non_int_operand() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::StringLiteral("three".to_string()),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range stop"));
        assert_eq!(env.lookup("i"), None);
    }

    #[test]
    fn a_for_range_loop_accepts_bool_as_an_int_subtype() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::BoolLiteral(false),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::BoolLiteral(true),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: loop variable is Maybe after the loop.
        assert_eq!(env.lookup("i"), None);
        assert_eq!(env.lookup_any("i"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_loop_binds_its_variable_as_the_list_element_type_and_checks_its_body() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("i".to_string()),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: the loop may execute zero times, so both the
        // loop variable `i` and body-only binding `y` are `Maybe`.
        assert_eq!(env.lookup("i"), None);
        assert_eq!(env.lookup("y"), None);
        assert_eq!(env.lookup_any("i"), Some(Ty::Int));
        assert_eq!(env.lookup_any("y"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_loop_binds_its_variable_as_str_for_a_list_of_str() {
        // Proves the `ForList` inference isn't hardcoded to `Ty::Int` --
        // it's generic over whatever scalar element type the list actually
        // has (see the `HirExpr::Subscript`/`ListAppend` genericity notes).
        // Bound directly rather than via a `str` list literal: T0034 gates
        // every non-`int` list literal at creation time, so no source-level
        // program can ever produce a live `Ty::List(Ty::Str)` binding --
        // this is the only way to reach this arm with one.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Str)));
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: loop variable is Maybe after the loop.
        assert_eq!(env.lookup("i"), None);
        assert_eq!(env.lookup_any("i"), Some(Ty::Str));
    }

    #[test]
    fn a_for_dict_loop_binds_its_variable_as_the_key_type() {
        // PR-11 Task 3 (D-123): `for k in d:` iterates a dict's own keys, so
        // `var` binds as the dict's key type (`Ty::Str`), not its value type.
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let stmt = HirStmt::ForList {
            var: "k".to_string(),
            list: "d".to_string(),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: loop variable is Maybe after the loop.
        assert_eq!(env.lookup("k"), None);
        assert_eq!(env.lookup_any("k"), Some(Ty::Str));
    }

    #[test]
    fn a_for_set_loop_binds_its_variable_as_the_element_type() {
        // PR-11 Task 7 (D-123): `for x in s:` iterates a set's own elements,
        // so `var` binds as the set's element type (`Ty::Int` for
        // `set[int]`).
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let stmt = HirStmt::ForList {
            var: "x".to_string(),
            list: "s".to_string(),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Issue #118 Part 1: loop variable is Maybe after the loop.
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
    }

    #[test]
    fn a_pre_bound_for_list_var_survives_as_definite() {
        // `i = 0; xs = [1, 2, 3]; for i in xs: pass` then `i` read --
        // succeeds because `i` was definitely bound before the loop
        // (issue #118 Part 1).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "i".to_string(),
                value: HirExpr::IntLiteral(0),
            },
        )
        .unwrap();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        check_stmt(
            &mut env,
            &HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![],
            },
        )
        .unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_loop_over_an_undefined_list_is_rejected() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "undefined".to_string(),
            body: vec![],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn a_for_list_loop_over_a_non_list_value_is_rejected() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(5),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "x".to_string(),
            body: vec![],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_for_list_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_list_loop_rejects_a_conflicting_reassignment_of_its_variable() {
        // `i` is already bound to `str` before the loop -- `check_assignment`
        // itself (not the list/element-type logic above it) is what rejects
        // rebinding it to the list's `int` element type.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "i".to_string(),
                value: HirExpr::StringLiteral("not an int".to_string()),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    // -- PR-12 Task 3 (D-117): comprehension type-checking, module scope --

    #[test]
    fn a_list_comprehension_over_range_type_checks_and_binds_target_as_list_int() {
        // Also pins that `var` is bound (via `check_assignment`) *before*
        // `elt` is checked: `elt` references the loop variable, so this
        // would fail as an unbound local if the ordering were wrong.
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::List(Box::new(Ty::Int))));
        assert_eq!(env.lookup("0comp_11_i"), Some(Ty::Int));
    }

    #[test]
    fn check_accepts_a_module_level_comprehension_whose_target_is_read_afterward() {
        // End-to-end pipeline pin, distinct from the hand-held-`Environment`
        // test above: every other comprehension test in this file either
        // drives `check_stmt`/`check_function` directly against a throwaway
        // `Environment`, or runs the full `check`/`check_and_resolve`
        // pipeline over a module expected to *fail*. This is the one test
        // confirming the full `check` pipeline (`module_function_names` ->
        // `concrete_function_environment` -> `check_with_environment` ->
        // `check_stmt`) accepts a *valid* module-level comprehension and
        // that its `target` binding genuinely survives into the next
        // top-level statement's environment, exactly like an ordinary
        // `Assign`'s binding already does.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "0comp_11_i".to_string(),
                    iter: CompIter::Range {
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![HirExpr::Name("xs".to_string())],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_list_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks() {
        // Mirrors `If`/`While`: any type is accepted as truthy, no static
        // bool-convertibility restriction.
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn a_list_comprehension_producing_str_is_rejected_as_t0034() {
        // Mirrors `ListLiteral`'s own existing genericity tests
        // (`a_homogeneous_non_int_list_literal_is_rejected_as_t0034`).
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[str]"));
    }

    #[test]
    fn a_list_comprehension_producing_float_is_rejected_as_t0034() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::FloatLiteral(1.5)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[float]"));
    }

    #[test]
    fn a_list_comprehension_producing_bool_is_rejected_as_t0034() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::BoolLiteral(true)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[bool]"));
    }

    #[test]
    fn a_list_comprehension_over_a_bare_list_name_type_checks_and_binds_target_as_list_int() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::List` (the `ListCompAssign` tests above all use
        // `CompIter::Range`, never a bare-name list iterable).
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_20_x".to_string(),
            iter: CompIter::Name("xs".to_string()),
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_20_x".to_string())),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::List(Box::new(Ty::Int))));
        assert_eq!(env.lookup("0comp_20_x"), Some(Ty::Int));
    }

    #[test]
    fn a_set_comprehension_over_a_bare_set_name_type_checks_and_binds_target_as_set_int() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Set`.
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_20_x".to_string(),
            iter: CompIter::Name("s".to_string()),
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_20_x".to_string())),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::Set(Box::new(Ty::Int))));
        assert_eq!(env.lookup("0comp_20_x"), Some(Ty::Int));
    }

    #[test]
    fn a_set_comprehension_producing_a_non_int_element_is_rejected_as_t0038() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0038");
        assert!(err.message.contains("set[str]"));
    }

    #[test]
    fn a_set_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::Set(Box::new(Ty::Int))));
    }

    #[test]
    fn a_dict_comprehension_over_a_bare_dict_name_type_checks_and_binds_target_as_dict_str_int() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Dict`, binding `var` as the dict's *key* type (mirroring
        // `ForList`'s own `for k in d:` behavior, D-123).
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_20_k".to_string(),
            iter: CompIter::Name("d".to_string()),
            cond: None,
            key: Box::new(HirExpr::Name("0comp_20_k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(
            env.lookup("y"),
            Some(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
        assert_eq!(env.lookup("0comp_20_k"), Some(Ty::Str));
    }

    #[test]
    fn a_dict_comprehension_producing_a_non_str_int_pair_is_rejected_as_t0036() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::IntLiteral(1)),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0036");
        assert!(err.message.contains("dict[int, int]"));
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(
            env.lookup("y"),
            Some(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
    }

    #[test]
    fn a_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033() {
        // Reuses `ForList`'s own existing message
        // (`a_for_list_loop_over_a_non_list_value_is_rejected`).
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("x".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_comprehension_over_an_undefined_iterable_name_is_rejected_as_not_defined() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("undefined".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }
    #[test]
    fn a_comprehension_over_a_maybe_bound_iterable_is_t0041() {
        // `if cond: xs = [1, 2, 3]` then `[x for x in xs]` -- xs is Maybe
        // after the if (no else), so `lookup_bound_name` raises T0041
        // (issue #118 Part 1).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                }],
                orelse: vec![],
            },
        )
        .unwrap();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("xs".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0041");
    }


    #[test]
    fn an_entirely_unannotated_private_helper_containing_a_comprehension_fails_with_t0021() {
        // Per D-116's own correction note: a container-literal assignment's
        // target never receives a solver binding at all (the solver only
        // unifies scalar `Ty::Infer` parameters/returns) -- a comprehension's
        // own `target` gets the identical treatment (Step 5's deliberate
        // no-op in `collect_block_constraints`). `def _h(): xs = (10, 20);
        // return xs` already fails T0021 "local name `t` is not bound before
        // this use" for a bare tuple/list assignment reaching a `Return` --
        // this reproduces the same shape with a comprehension in place of a
        // literal, confirming it is the same pre-existing gap, not a new one
        // introduced by this statement.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "xs".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                    },
                    HirStmt::Return(Some(HirExpr::Name("xs".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    // `resolve_comp_iter`'s own `CompIter::Range` operand checks (shared by
    // every comprehension kind and both scopes) -- exercised once here via
    // a list comprehension, mirroring `a_for_range_loop_whose_start_is_
    // undefined_propagates_the_error` and its stop/step siblings.

    #[test]
    fn a_comprehension_over_a_range_with_a_non_int_start_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::StringLiteral("x".to_string()),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range start expects"));
    }

    #[test]
    fn a_comprehension_over_a_range_with_a_non_int_stop_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::StringLiteral("x".to_string()),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range stop expects"));
    }

    #[test]
    fn a_comprehension_over_a_range_with_a_non_int_step_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::StringLiteral("x".to_string()),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range step expects"));
    }

    // Per-arm error-propagation pins (module scope): each of `resolve_comp_iter`'s
    // call, the loop-variable `check_assignment`, `cond`, and `elt`/`key`/`value`
    // is its own `?`-guarded call site in each of the three comprehension arms,
    // so each needs its own error-propagating test for D-014's coverage gate,
    // distinct from the "rejected with a specific diagnostic" tests above.

    #[test]
    fn a_set_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("x".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_dict_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("x".to_string()),
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_list_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable() {
        let mut env = Environment::new();
        env.bind("0comp_11_i".to_string(), Ty::Str);
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_set_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable() {
        let mut env = Environment::new();
        env.bind("0comp_11_i".to_string(), Ty::Str);
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_dict_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable() {
        let mut env = Environment::new();
        env.bind("0comp_11_i".to_string(), Ty::Str);
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_list_comprehension_s_if_filter_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_if_filter_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_list_comprehension_s_elt_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_elt_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_key_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::Name("undefined".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_value_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn referencing_an_assigned_name_infers_its_bound_type() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        assert_eq!(
            infer_expr(&env, &HirExpr::Name("x".to_string())),
            Ok(Ty::Int)
        );
    }

    #[test]
    fn adding_two_ints_infers_int() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_binop_with_an_undefined_left_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::Name("undefined".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_binop_with_an_undefined_right_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Name("undefined".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn numeric_result_type_covers_every_int_float_combination() {
        assert_eq!(
            numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Float),
            Ok(Ty::Float)
        );
        assert_eq!(
            numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Int),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn true_division_of_two_ints_infers_float() {
        assert_eq!(
            numeric_result_type(BinOpKind::Div, Ty::Int, Ty::Int),
            Ok(Ty::Float)
        );
        assert_eq!(
            numeric_result_type(BinOpKind::Div, Ty::Bool, Ty::Bool),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn floor_division_of_two_ints_still_infers_int() {
        assert_eq!(
            numeric_result_type(BinOpKind::FloorDiv, Ty::Int, Ty::Int),
            Ok(Ty::Int)
        );
    }

    #[test]
    fn referencing_an_undefined_name_is_a_clean_error_not_a_panic() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::Name("undefined".to_string())).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined"));
    }

    #[test]
    fn numeric_result_type_rejects_a_hypothetical_incompatible_pair() {
        let err = numeric_result_type(BinOpKind::Add, Ty::Int, Ty::None).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn adding_an_int_and_a_float_promotes_to_float() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::FloatLiteral(2.5)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn numeric_result_type_accepts_float_and_bool_since_bool_is_numeric_like() {
        // Task 7 makes `bool` numeric-like everywhere (`True + 1.5 == 2.5` is
        // legal Python), so this pair is no longer an error -- see
        // `a_binop_treats_bool_and_float_as_float` for the `infer_expr`-level
        // version of this same rule.
        assert_eq!(
            numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Bool),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn numeric_result_type_rejects_a_float_and_a_hypothetical_none() {
        // Exercises `.name()` for `Float` in the error arm now that
        // `Float`+`Bool` no longer takes that path.
        let err = numeric_result_type(BinOpKind::Add, Ty::Float, Ty::None).unwrap_err();
        assert!(err.message.contains("float") && err.message.contains("None"));
    }

    #[test]
    fn numeric_result_type_rejects_a_hypothetical_str_operand() {
        let err = numeric_result_type(BinOpKind::Add, Ty::Bool, Ty::Str).unwrap_err();
        assert!(err.message.contains("str"));
    }

    #[test]
    fn an_empty_list_literal_cannot_be_inferred() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::ListLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty list literal"));
    }

    #[test]
    fn a_homogeneous_int_list_literal_infers_list_of_int() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn a_heterogeneous_list_literal_is_rejected_as_t0032() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::StringLiteral("two".to_string()),
            HirExpr::IntLiteral(3),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0032");
        // Exact match, not a substring check: this is the same message text
        // baked into `tests/diagnostics/d0032_heterogeneous_list_literal
        // .expected.txt`, which can't currently run end-to-end (`pycc_mir`
        // is still non-exhaustive against Task 7's HIR forms) -- this unit
        // test is what actually guards that fixture's wording today.
        assert_eq!(
            err.message,
            "list element type mismatch: expected int (from the first element), found str"
        );
    }

    #[test]
    fn a_list_literal_of_bool_and_int_is_rejected_since_homogeneity_uses_exact_ty_equality() {
        // D-105 requires the *exact same* `Ty` for every element -- unlike
        // `is_assignable`'s bool-is-an-int-subtype rule used elsewhere in
        // this file, `[1, True]` is still T0032.
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::BoolLiteral(true)]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0032");
    }

    #[test]
    fn a_homogeneous_non_int_list_literal_is_rejected_as_t0034() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::StringLiteral("b".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        // Exact match: same wording as
        // `tests/diagnostics/d0034_list_element_type_not_int.expected.txt`
        // (currently unrunnable end-to-end -- see the T0032 test above).
        assert_eq!(
            err.message,
            "list[str] is not compiled yet (D-105) -- only list[int] is"
        );
    }

    #[test]
    fn a_list_literal_propagates_an_ill_typed_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![HirExpr::Name("undefined".to_string())]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn subscripting_a_list_infers_its_element_type() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn subscripting_a_list_of_str_infers_str_proving_subscript_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn subscripting_with_a_bool_index_is_accepted_since_bool_is_an_int_subtype() {
        // D-086: `bool` is accepted wherever `int` is expected at an operand
        // boundary (mirroring `is_assignable`'s existing param/assignment
        // rule) -- `xs[True]` is ordinary, CPython-valid Python (PEP 285),
        // not a type error. Found by an automated PR review (PR #236) and
        // confirmed against a real `pycc build` before this fix.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn subscripting_with_a_non_int_index_is_rejected() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::StringLiteral("zero".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("list index must be"));
    }

    #[test]
    fn subscripting_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        // Exact match: same wording as
        // `tests/diagnostics/d0033_subscript_on_non_list.expected.txt`
        // (currently unrunnable end-to-end -- see the T0032 test above).
        assert_eq!(err.message, "`int` does not support indexing");
    }

    #[test]
    fn subscripting_a_non_list_value_with_a_non_int_index_reports_t0033_not_t0021() {
        // Both operands are ill-typed at once: the base doesn't support
        // indexing at all, AND the index isn't int-compatible. T0033
        // ("does this value support the operation at all") must fire
        // before T0021 ("is this operand's type wrong"). The `match
        // base_ty { Ty::List(..) => ..., Ty::Dict(..) => ..., Ty::Tuple(..)
        // => ..., other => T0033 }` dispatch above already gives this
        // precedence structurally -- a non-container base falls straight
        // to the `other` arm without ever reaching a container's own
        // index-type check -- but PR-10's own final whole-branch review
        // found and pinned this exact compound case as a regression test
        // before container types existed, so it is kept here too.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::StringLiteral("zero".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`int` does not support indexing");
    }

    #[test]
    fn subscripting_propagates_an_ill_typed_base_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("undefined".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn subscripting_propagates_an_ill_typed_index_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // PR-12 Task 7 (D-118): `HirExpr::Slice` type-checking. Only
    // `list[int]` ships slicing in v0.2 -- see the arm's own doc comment in
    // `infer_expr_in` for the full base-type/element-type/bound-type
    // diagnostic-order rationale.

    #[test]
    fn slicing_a_list_of_int_with_both_bounds_infers_list_of_int() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_start_omitted_type_checks() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: Some(Box::new(HirExpr::IntLiteral(1))),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_stop_omitted_type_checks() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: None,
            step: Some(Box::new(HirExpr::IntLiteral(1))),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_step_omitted_type_checks() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_every_bound_omitted_type_checks_as_xs_colon() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_a_dict_is_rejected_as_t0033() {
        // Mirrors real CPython: `d[1:2]` raises `TypeError` there too (D-118
        // -- dict/set reuse `Subscript`'s own `T0033` code, not a new one).
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("d".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`dict[str, int]` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_set_is_rejected_as_t0033() {
        // Mirrors real CPython: `s[1:2]` raises `TypeError` there too.
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("s".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`set[int]` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_tuple_is_rejected_as_t0033_as_an_explicit_deferral() {
        // D-118: unlike dict/set (never supported for slicing), `tuple[...]`
        // slicing is a real, explicit v0.2 scope cut -- but it still reuses
        // the same `T0033` code, not a distinct one.
        let mut env = Environment::new();
        env.bind("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("t".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`tuple[int, int]` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_scalar_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("x".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`int` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_list_of_str_is_rejected_as_t0034() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert_eq!(
            err.message,
            "list codegen only supports `list[int]` in v0.2, cannot slice `list[str]`"
        );
    }

    #[test]
    fn slicing_a_list_of_float_is_rejected_as_t0034() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Float)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert_eq!(
            err.message,
            "list codegen only supports `list[int]` in v0.2, cannot slice `list[float]`"
        );
    }

    #[test]
    fn slicing_a_list_of_bool_is_rejected_as_t0034() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Bool)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert_eq!(
            err.message,
            "list codegen only supports `list[int]` in v0.2, cannot slice `list[bool]`"
        );
    }

    #[test]
    fn slicing_with_a_non_int_start_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::StringLiteral("a".to_string()))),
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "slice start must be `int`, got `str`");
    }

    #[test]
    fn slicing_with_a_non_int_stop_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: Some(Box::new(HirExpr::FloatLiteral(1.0))),
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "slice stop must be `int`, got `float`");
    }

    #[test]
    fn slicing_with_a_non_int_step_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: Some(Box::new(HirExpr::StringLiteral("a".to_string()))),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "slice step must be `int`, got `str`");
    }

    #[test]
    fn slicing_with_a_bool_bound_is_accepted_since_bool_is_an_int_subtype() {
        // D-086, mirroring `Subscript`'s own index check: `xs[True:]` is
        // ordinary, CPython-valid Python (`bool` is an `int` subtype, PEP
        // 285), not a type error.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::BoolLiteral(true))),
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_reports_the_base_type_error_before_any_bound_error() {
        // D-118 diagnostic-order pin: the base-type gate (T0033) fires
        // before a bound's own type is ever inspected.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("x".to_string())),
            start: Some(Box::new(HirExpr::StringLiteral("bad".to_string()))),
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
    }

    #[test]
    fn slicing_reports_the_element_type_error_before_any_bound_error() {
        // D-118 diagnostic-order pin: the element-type gate (T0034) fires
        // before a bound's own type is ever inspected.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::StringLiteral("bad".to_string()))),
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
    }

    #[test]
    fn slicing_propagates_an_ill_typed_base_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("undefined".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn slicing_propagates_an_ill_typed_bound_s_error() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn appending_a_matching_value_to_a_list_infers_none() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn appending_a_str_to_a_list_of_str_infers_none_proving_append_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::StringLiteral("a".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn appending_to_an_undefined_name_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::ListAppend {
            list: "undefined".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn appending_to_a_local_name_read_before_assignment_is_unbound_local() {
        // `ListAppend`'s `list` field is a plain `String`, not an
        // `HirExpr::Name`, so it must replicate `HirExpr::Name`'s own
        // "unbound local" vs. "not defined" distinction by hand
        // (`lookup_bound_name`) rather than going through `infer_expr_in`'s
        // `Name` arm.
        let env = Environment::new();
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr_in(&env, &["x"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn appending_to_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.append()`"));
    }

    #[test]
    fn appending_a_mismatched_value_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::StringLiteral("nope".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot append"));
    }

    #[test]
    fn appending_a_bool_to_a_list_of_int_is_accepted_since_bool_is_an_int_subtype() {
        // D-086, matching Subscript's own index check above: `x.append(True)`
        // on an already-typed `list[int]` is ordinary, CPython-valid Python
        // (`bool` is an `int` subtype) -- `is_assignable` applies here, not
        // ListLiteral's stricter exact-equality homogeneity rule (which
        // answers a different question: inferring an as-yet-unknown element
        // type, not checking a value against an already-known one). Found
        // by an automated whole-branch review (PR #236) and confirmed
        // against a real `pycc build` before this fix.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn appending_propagates_an_ill_typed_value_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // -- PR-12 Task 10 (D-119): remaining container methods depth --------
    // `list.pop()`, `dict.get(key, default)`, `set.add(value)` -- each
    // mirrors `ListAppend`'s own `infer_expr_in` test coverage exactly
    // (base-type gate, value/key/default-type gate, `lookup_bound_name`'s
    // own "not defined"/"unbound local" distinction, propagation, D-086
    // bool-subtypes-int leniency, genericity).

    #[test]
    fn popping_the_last_element_of_a_list_of_int_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn popping_from_a_list_of_str_infers_str_proving_pop_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn popping_from_an_undefined_name_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::ListPop {
            list: "undefined".to_string(),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn popping_from_a_local_name_read_before_assignment_is_unbound_local() {
        let env = Environment::new();
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        let err = infer_expr_in(&env, &["x"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn popping_from_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.pop()`"));
    }

    #[test]
    fn getting_a_str_key_with_a_matching_int_default_infers_int() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn getting_a_bool_default_for_a_dict_of_int_values_is_accepted_since_bool_is_an_int_subtype() {
        // D-086, matching `ListAppend`'s own leniency: `bool` is an `int`
        // subtype, so a `bool` default for a `dict[str, int]` is ordinary,
        // CPython-valid Python, not a type error.
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn getting_from_an_undefined_dict_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::DictGetOrDefault {
            dict: "undefined".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn getting_from_a_local_dict_read_before_assignment_is_unbound_local() {
        let env = Environment::new();
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr_in(&env, &["d"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn getting_from_a_non_dict_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Int);
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.get()`"));
    }

    #[test]
    fn getting_with_a_mismatched_key_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::IntLiteral(1)),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot look up"));
    }

    #[test]
    fn getting_with_a_mismatched_default_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::StringLiteral("nope".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot use"));
    }

    #[test]
    fn getting_propagates_an_ill_typed_key_s_error() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::Name("undefined".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn getting_propagates_an_ill_typed_default_s_error() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn adding_an_int_value_to_a_set_of_int_infers_none() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn adding_a_bool_value_to_a_set_of_int_is_accepted_since_bool_is_an_int_subtype() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn adding_a_value_to_a_set_of_str_infers_none_proving_add_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Str)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::StringLiteral("a".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn adding_to_an_undefined_set_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::SetAdd {
            set: "undefined".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn adding_to_a_local_set_read_before_assignment_is_unbound_local() {
        let env = Environment::new();
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr_in(&env, &["s"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn adding_to_a_non_set_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Int);
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.add()`"));
    }

    #[test]
    fn adding_a_mismatched_value_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::StringLiteral("nope".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot add"));
    }

    #[test]
    fn adding_propagates_an_ill_typed_value_s_error() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // Whole-module `check()` tests, mirroring `HirExpr::Slice`'s own pair
    // (PR-12 Task 7, D-118) exactly: Task 3's own ledger records a real
    // regression of exactly this shape (a solver arm that looked correct in
    // isolation but was never actually reached by the block walker) -- these
    // confirm each new arm is genuinely wired into both the fast
    // (`check_with_environment`) and solver (`collect_block_constraints`)
    // paths, not just correct when `infer_expr`/`collect_expr_constraints`
    // are called directly on a hand-built expression.

    #[test]
    fn list_pop_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::ListPop {
                        list: "xs".to_string(),
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn dict_get_or_default_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path()
     {
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
                        key: Box::new(HirExpr::StringLiteral("a".to_string())),
                        default: Box::new(HirExpr::IntLiteral(0)),
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn set_add_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path() {
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
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn list_pop_on_a_list_typed_parameter_infers_the_scalar_element_type_inside_an_unannotated_private_helper()
     {
        // D-146 (#239): `collect_expr_constraints`'s `ListPop` arm now looks
        // up `list` in `env.bindings` and, when bound to a `Ty::List` element-
        // type carrier, extracts the scalar element type -- the carrier is
        // destructured, never unified. Here `xs` is a parameter typed
        // `list[int]`, so `xs.pop()` produces `Some(Ok(Ty::Int))`, `y` is
        // bound to `Ok(Ty::Int)`, and `return y` resolves the unannotated
        // return to `Ty::Int`. This was the pre-existing D-116 solver binding
        // gap: before this fix the `ListPop` arm returned `Ok(None)`, so `y`
        // was never bound and the later `return y` failed with the actively
        // misleading "not bound before this use" even though the assignment
        // was textually right above it.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::Assign {
                        target: "y".to_string(),
                        value: HirExpr::ListPop {
                            list: "xs".to_string(),
                        },
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let signatures = infer_function_signatures_with_solver(&hir, &local_names).unwrap();
        assert_eq!(signatures["_h"], (vec![Ty::List(Box::new(Ty::Int))], Ty::Int));
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn dict_get_or_default_assigned_inside_an_unannotated_private_helper_hits_the_pre_existing_solver_binding_gap_today()
     {
        // Same pre-existing D-116 gap as the `ListPop` test immediately
        // above, exercised through `DictGetOrDefault`'s own
        // `collect_expr_constraints` arm (also `Ok(None)`) instead.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))))],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::Assign {
                        target: "y".to_string(),
                        value: HirExpr::DictGetOrDefault {
                            dict: "d".to_string(),
                            key: Box::new(HirExpr::StringLiteral("a".to_string())),
                            default: Box::new(HirExpr::IntLiteral(0)),
                        },
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn len_of_a_list_of_int_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn len_of_a_list_of_str_infers_int_proving_len_is_not_int_specific() {
        // Proves `len`'s own check isn't hardcoded to `Ty::Int` -- generic
        // over any scalar element type, same discipline already applied to
        // `Subscript`/`ListAppend`/`ForList` (D-105's own genericity claim).
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn len_with_no_arguments_is_rejected_as_t0033() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`len` expects exactly 1 argument, got 0");
    }

    #[test]
    fn len_with_two_arguments_is_rejected_as_t0033() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`len` expects exactly 1 argument, got 2");
    }

    #[test]
    fn len_of_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `int`"
        );
    }

    #[test]
    fn len_propagates_an_ill_typed_argument_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn float_of_an_int_infers_float() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn float_of_a_bool_infers_float() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Bool);
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn float_of_a_float_infers_float() {
        // Proves `float`'s own check isn't a narrowing-only conversion --
        // an already-`float` argument is accepted too (identity), same
        // discipline as `len_of_a_list_of_str_infers_int_proving_len_is_
        // not_int_specific`'s own genericity claim above.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Float);
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn float_with_no_arguments_is_rejected_as_t0021() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`float` expects exactly 1 argument, got 0");
    }

    #[test]
    fn float_with_two_arguments_is_rejected_as_t0021() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`float` expects exactly 1 argument, got 2");
    }

    #[test]
    fn float_of_a_str_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Str);
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "`float` expects an `int`, `float`, or `bool` argument, got `str`"
        );
    }

    #[test]
    fn float_propagates_an_ill_typed_argument_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn math_sqrt_and_pi_type_check_through_the_constraint_solver_path() {
        // `math.sqrt`/`math.pi`'s `infer_expr_in` coverage above only
        // exercises pass 3 (the final, concrete-environment check). A
        // private helper with an unannotated (`Ty::Infer`) return forces
        // `collect_expr_constraints` (the solver pass) to resolve the same
        // call/name -- this is `inferred_signatures_keep_the_constraint_
        // solver_path`'s own precedent, applied to the new stdlib branch.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_root_plus_pi".to_string(),
                // `x` is annotated (not `Ty::Infer`) deliberately: like
                // `float`/`len`, `math.sqrt`'s solver-pass branch only
                // validates an already-concretely-resolved argument term
                // (see that branch's own doc comment on the "lenient-
                // until-known" precedent) -- it does not itself unify an
                // unresolved parameter's inference variable against the
                // stdlib signature's expected argument type. Only the
                // function's own `Ty::Infer` *return* is left for the
                // solver to determine here.
                params: vec![("x".to_string(), Ty::Float)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Call {
                        callee: "math.sqrt".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    }),
                    right: Box::new(HirExpr::Name("math.pi".to_string())),
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let signatures = infer_function_signatures_with_solver(&hir, &local_names).unwrap();
        assert_eq!(signatures["_root_plus_pi"], (vec![Ty::Float], Ty::Float));
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn math_sqrt_wrong_arity_is_rejected_by_the_constraint_solver_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad_call".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "math.sqrt".to_string(),
                    args: vec![],
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn math_sqrt_wrong_argument_type_is_rejected_by_the_constraint_solver_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad_call".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "math.sqrt".to_string(),
                    args: vec![HirExpr::StringLiteral("x".to_string())],
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn math_pi_called_like_a_function_is_rejected_by_the_constraint_solver_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad_call".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "math.pi".to_string(),
                    args: vec![],
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn math_sqrt_used_as_a_bare_value_is_rejected_by_the_constraint_solver_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad_ref".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("math.sqrt".to_string())))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_dotted_name_whose_module_part_is_not_a_registered_stdlib_module_is_an_ordinary_undefined_name() {
        // `std_qualified_symbol`'s `resolve_module` step is defense-in-depth
        // against a hand-constructed `HirModule` (this crate's own public
        // API accepts any `&HirModule`, not only one `pycc_hir::lower_checked`
        // produced) -- `pycc_hir`'s real lowering never emits a dotted name
        // whose module part fails to resolve (see `lower_expr`'s own
        // `Expr::Attribute` arm), but this crate does not get to assume
        // that invariant holds for every caller.
        let env = Environment::new();
        let expr = HirExpr::Name("os.path".to_string());
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `os.path` is not defined");
    }

    #[test]
    fn math_sqrt_of_a_float_infers_float() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "math.sqrt".to_string(),
            args: vec![HirExpr::FloatLiteral(2.0)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn math_pi_infers_float() {
        let env = Environment::new();
        let expr = HirExpr::Name("math.pi".to_string());
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn math_sqrt_with_no_arguments_is_rejected_as_t0021() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "math.sqrt".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`math.sqrt` expects 1 argument(s), got 0");
    }

    #[test]
    fn math_sqrt_of_a_str_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Str);
        let expr = HirExpr::Call {
            callee: "math.sqrt".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "`math.sqrt` expects `float`, got `str`");
    }

    #[test]
    fn math_sqrt_propagates_an_ill_typed_argument_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "math.sqrt".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn math_pi_called_like_a_function_is_rejected_as_t0021() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "math.pi".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "`math.pi` is a stdlib constant, not a function, and cannot be called"
        );
    }

    #[test]
    fn math_sqrt_used_as_a_bare_value_is_rejected_as_t0021() {
        let env = Environment::new();
        let expr = HirExpr::Name("math.sqrt".to_string());
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "`math.sqrt` is a stdlib function and must be called, e.g. `math.sqrt(...)`"
        );
    }

    #[test]
    fn math_sqrt_call_is_shadowed_by_a_bound_local_named_math() {
        // Post-review finding: a real local/parameter legally named `math`
        // must shadow the stdlib module -- CPython would raise
        // `AttributeError` calling `.sqrt` on whatever `math` is actually
        // bound to here, not silently call libm's `sqrt`.
        let err = infer_expr_in(
            &Environment::new(),
            &["math"],
            &HirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![HirExpr::FloatLiteral(2.0)],
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert_eq!(
            err.message,
            "`math` is a local name here, not the stdlib `math` module -- attribute access on a non-module value is not supported yet"
        );
    }

    #[test]
    fn math_pi_reference_is_shadowed_by_a_bound_local_named_math() {
        let err = infer_expr_in(
            &Environment::new(),
            &["math"],
            &HirExpr::Name("math.pi".to_string()),
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert_eq!(
            err.message,
            "`math` is a local name here, not the stdlib `math` module -- attribute access on a non-module value is not supported yet"
        );
    }

    #[test]
    fn math_sqrt_call_is_shadowed_by_a_module_level_binding_named_math() {
        let mut env = Environment::new();
        env.bind("math".to_string(), Ty::Float);
        let err = infer_expr(
            &env,
            &HirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![HirExpr::FloatLiteral(2.0)],
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn math_sqrt_and_pi_are_shadowed_by_a_bound_local_through_the_constraint_solver_path() {
        // The other half of the fix -- the private-helper solver pass
        // (`collect_expr_constraints`) needs the same shadowing guard as
        // `infer_expr_in` above.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_shadowed".to_string(),
                params: vec![("math".to_string(), Ty::Float)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "math.sqrt".to_string(),
                    args: vec![HirExpr::Name("math".to_string())],
                }))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn math_pi_bare_reference_is_shadowed_by_a_bound_local_through_the_constraint_solver_path() {
        // The `HirExpr::Name` (bare-reference) half of the solver-path
        // shadowing guard, as opposed to the `HirExpr::Call` half already
        // covered by `math_sqrt_and_pi_are_shadowed_by_a_bound_local_
        // through_the_constraint_solver_path` above.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_shadowed_pi".to_string(),
                params: vec![("math".to_string(), Ty::Float)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("math.pi".to_string())))],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn a_user_defined_float_function_takes_priority_over_the_builtin() {
        // Post-merge review finding: unlike `len`/`print`, which have been
        // hand-recognized since before this compiler could compile
        // user-declared functions at all, `float` was undefined until #181,
        // so `def float(x: int) -> int: return x + 1` was a valid, working
        // program on `main` immediately before this builtin landed --
        // reproduced directly against a pristine `main` checkout, printing
        // `6`. Without this priority check, the builtin would silently
        // intercept the call and infer `Ty::Float` instead of the user's own
        // declared `Ty::Int` return type.
        let mut env = Environment::new();
        env.bind_function("float".to_string(), vec![Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::IntLiteral(5)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_user_defined_float_function_accepting_a_non_numeric_argument_type_checks() {
        // The other half of the same finding: the builtin's own argument-type
        // gate (`int`/`float`/`bool` only) must not apply when the user's own
        // `float` accepts something else entirely, e.g. `str`.
        let mut env = Environment::new();
        env.bind_function("float".to_string(), vec![Ty::Str], Ty::Str);
        let expr = HirExpr::Call {
            callee: "float".to_string(),
            args: vec![HirExpr::StringLiteral("hello".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    // -- Issue #142: callable builtin classification as C0001 ------------

    #[test]
    fn known_callable_builtins_table_is_sorted() {
        // The `is_known_callable_builtin` binary search relies on
        // `KNOWN_CALLABLE_BUILTINS` being sorted by Rust's `str` ordering.
        // If this invariant is ever violated, binary search silently returns
        // false negatives -- a known builtin would be misclassified as
        // T0021 instead of C0001. The explicit `let` bindings ensure both
        // values are always evaluated (coverage), while the `assert!`
        // without format args avoids an uncovered panic-message branch.
        for w in KNOWN_CALLABLE_BUILTINS.windows(2) {
            let prev = w[0];
            let next = w[1];
            assert!(prev <= next, "KNOWN_CALLABLE_BUILTINS is not sorted");
            // Strict ordering (no duplicates): `prev < next` is the real
            // invariant, but `prev <= next` above already covers both
            // values. This redundant strict check catches duplicates without
            // introducing an uncovered format-arg branch.
            assert!(prev != next, "KNOWN_CALLABLE_BUILTINS has a duplicate");
        }
    }

    #[test]
    fn known_callable_builtins_table_has_137_entries() {
        assert_eq!(
            KNOWN_CALLABLE_BUILTINS.len(),
            137,
            "KNOWN_CALLABLE_BUILTINS should have 137 entries (139 builtins minus print/len/float, plus __import__)"
        );
    }

    #[test]
    fn known_callable_builtins_excludes_already_implemented() {
        // `print`, `len`, and `float` are already hand-recognized and must
        // not appear in the table -- they would never reach this fallback.
        assert!(!is_known_callable_builtin("print"));
        assert!(!is_known_callable_builtin("len"));
        assert!(!is_known_callable_builtin("float"));
    }

    #[test]
    fn is_known_callable_builtin_finds_table_entries() {
        // Representative samples from both halves of the table (exception
        // classes and ordinary builtins).
        assert!(is_known_callable_builtin("ValueError"));
        assert!(is_known_callable_builtin("Exception"));
        assert!(is_known_callable_builtin("int"));
        assert!(is_known_callable_builtin("range"));
        assert!(is_known_callable_builtin("zip"));
        assert!(is_known_callable_builtin("ArithmeticError"));
        assert!(is_known_callable_builtin("ZeroDivisionError"));
    }

    #[test]
    fn is_known_callable_builtin_rejects_unknown_names() {
        assert!(!is_known_callable_builtin("totally_undefined"));
        assert!(!is_known_callable_builtin("print"));
        assert!(!is_known_callable_builtin(""));
    }

    #[test]
    fn value_error_call_produces_c0001_not_t0021() {
        // `ValueError("x")` is valid Python -- the builtin genuinely exists
        // in Python 3.14 -- so it must be classified as a capability gap
        // (C0001), not a name-resolution failure (T0021).
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "ValueError".to_string(),
            args: vec![HirExpr::StringLiteral("x".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("ValueError"));
    }

    #[test]
    fn exception_call_produces_c0001() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "Exception".to_string(),
            args: vec![HirExpr::StringLiteral("msg".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("Exception"));
    }

    #[test]
    fn other_callable_builtins_produce_c0001() {
        // A representative sample of other callable builtins from the table.
        for name in ["int", "str", "bool", "range", "zip", "dict", "list", "sum"] {
            let env = Environment::new();
            let expr = HirExpr::Call {
                callee: name.to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            };
            let err = infer_expr(&env, &expr).unwrap_err();
            assert_eq!(err.code, "C0001", "builtin `{name}` should be C0001");
            assert!(err.message.contains(name));
        }
    }

    #[test]
    fn a_genuinely_undefined_function_still_produces_t0021() {
        // A typo or genuinely undefined name must retain the T0021
        // name-resolution behavior, not be reclassified as C0001.
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "totally_undefined".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined function"));
    }

    #[test]
    fn a_user_defined_value_error_takes_priority_over_c0001() {
        // A user `def ValueError(...)` is called correctly, not classified as
        // C0001 -- user definitions always take priority over the builtin
        // classification, matching `float`'s own user-definition-takes-
        // priority precedent.
        let mut env = Environment::new();
        env.bind_function("ValueError".to_string(), vec![Ty::Str], Ty::Int);
        let expr = HirExpr::Call {
            callee: "ValueError".to_string(),
            args: vec![HirExpr::StringLiteral("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_user_defined_exception_takes_priority_over_c0001() {
        let mut env = Environment::new();
        env.bind_function("Exception".to_string(), vec![Ty::Str], Ty::Bool);
        let expr = HirExpr::Call {
            callee: "Exception".to_string(),
            args: vec![HirExpr::StringLiteral("msg".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn range_as_a_standalone_call_produces_c0001() {
        // `range` works in `for` loops (via `HirStmt::ForRange`) but not as
        // a standalone call -- a standalone `range(10)` is C0001.
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "range".to_string(),
            args: vec![HirExpr::IntLiteral(10)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("range"));
    }

    #[test]
    fn import_dunder_call_produces_c0001_not_t0021() {
        // `__import__("math")` is valid Python -- the callable dunder
        // `__import__` is the one dunder users legitimately call, so it
        // is included in the builtin table. It must be classified as C0001,
        // not T0021 (Codex review P2).
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "__import__".to_string(),
            args: vec![HirExpr::StringLiteral("math".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("__import__"));
    }

    #[test]
    fn constraint_collection_classifies_value_error_as_c0001() {
        // The private-helper inference path (`collect_expr_constraints`)
        // must apply the same C0001 classification for a known callable
        // builtin, rather than deferring with `Ok(None)` -- a private
        // helper calling `ValueError` gets C0001 directly.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "ValueError".to_string(),
            args: vec![HirExpr::StringLiteral("x".to_string())],
        };
        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("ValueError"));
    }

    #[test]
    fn constraint_collection_classifies_exception_as_c0001() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "Exception".to_string(),
            args: vec![HirExpr::StringLiteral("msg".to_string())],
        };
        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("Exception"));
    }

    #[test]
    fn constraint_collection_defers_unknown_callees_to_final_validation() {
        // A genuinely unknown callee still returns `Ok(None)` and defers to
        // final validation's T0021 -- the C0001 classification only applies
        // to known callable builtins.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "totally_undefined".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        };
        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();
        assert_eq!(term, None);
    }

    #[test]
    fn constraint_collection_honors_user_defined_value_error_over_c0001() {
        // A registered `ValueError` signature resolves through normal
        // signature lookup, not the C0001 builtin classification -- user
        // definitions take priority in the private-helper path too.
        let signatures = HashMap::from([(
            "ValueError".to_string(),
            (vec!["x".to_string()], vec![Ok(Ty::Str)], Ok(Ty::Int)),
        )]);
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "ValueError".to_string(),
            args: vec![HirExpr::StringLiteral("x".to_string())],
        };
        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();
        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    // -- PR-11 Task 3 (D-123): dict[str, int] type-checking --------------

    #[test]
    fn an_empty_dict_literal_cannot_be_inferred() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::DictLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty dict literal"));
    }

    #[test]
    fn a_homogeneous_dict_literal_infers_dict_of_str_int() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::StringLiteral("b".to_string()),
                HirExpr::IntLiteral(2),
            ),
        ]);
        assert_eq!(
            infer_expr(&env, &expr),
            Ok(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
    }

    #[test]
    fn a_dict_literal_with_mismatched_value_types_is_rejected_as_t0035() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::StringLiteral("b".to_string()),
                HirExpr::StringLiteral("oops".to_string()),
            ),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0035");
        assert_eq!(
            err.message,
            "dict entry type mismatch: expected str: int (from the first pair), found str: str"
        );
    }

    #[test]
    fn a_dict_literal_with_mismatched_key_types_is_rejected_as_t0035() {
        // Same code as the value-mismatch case above, but exercises the
        // `this_key_ty != key_ty` half of the homogeneity check's `||`
        // independently of the value half.
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (HirExpr::IntLiteral(2), HirExpr::IntLiteral(2)),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0035");
        assert_eq!(
            err.message,
            "dict entry type mismatch: expected str: int (from the first pair), found int: int"
        );
    }

    #[test]
    fn a_homogeneous_non_str_int_dict_literal_is_rejected_as_t0036() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::StringLiteral("b".to_string()),
        )]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0036");
        assert_eq!(
            err.message,
            "dict[str, str] is not compiled yet (D-122) -- only dict[str, int] is"
        );
    }

    #[test]
    fn a_bad_dict_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The dict-shaped counterpart to
        // `a_heterogeneous_list_literal_is_still_rejected_when_the_solver_path_runs_first`
        // above: `collect_expr_constraints`'s own `DictLiteral` arm always returns
        // `Ok(None)` (it has no unification-friendly representation for `Ty::Dict`
        // either), so an unrelated private helper with an unresolved (`Ty::Infer`)
        // signature must not let the solver's own leniency swallow a genuine T0036 --
        // it has to fall through to the real, dict-aware check pass.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::StringLiteral("b".to_string()),
                    )]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0036");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_first_key_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::Name("undefined".to_string()),
            HirExpr::IntLiteral(1),
        )]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_first_value_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::Name("undefined".to_string()),
        )]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_later_key_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::Name("undefined".to_string()),
                HirExpr::IntLiteral(2),
            ),
        ]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_later_value_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::StringLiteral("b".to_string()),
                HirExpr::Name("undefined".to_string()),
            ),
        ]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn subscripting_a_dict_infers_its_value_type() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::StringLiteral("a".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn subscripting_a_dict_with_a_mismatched_key_type_is_rejected() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "dict key type mismatch: expected `str`, found `int`"
        );
    }

    #[test]
    fn len_of_a_dict_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn constraint_collection_len_call_returns_int_for_a_concretely_bound_dict() {
        // Mirrors `constraint_collection_len_call_returns_int_for_a_concretely_bound_list`
        // above, proving the solver's own relaxed `len()` arm (PR-11 Task 3)
        // also accepts a concretely-bound `Ty::Dict` term.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([(
                "d".to_string(),
                Ok(Ty::Dict(Box::new((Ty::Str, Ty::Int)))),
            )]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("d".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_treats_a_dict_literal_as_unconstrained_but_recurses_into_pairs() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::IntLiteral(1),
        )]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_literal_key() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::Name("missing".to_string()),
            HirExpr::IntLiteral(1),
        )]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_literal_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::Name("missing".to_string()),
        )]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn collect_block_constraints_recurses_into_a_dict_set_s_key_and_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_set_key() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::Name("missing".to_string()),
            value: HirExpr::IntLiteral(1),
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_set_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::Name("missing".to_string()),
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn local_name_collection_ignores_a_dict_set_target() {
        // `d[k] = v` mutates an existing binding's contents, not a name --
        // unlike `Assign`/`AnnAssign`/`ForList`, it must not be collected as
        // a new local.
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];
        assert_eq!(function_local_names(&params, &body), Vec::<&str>::new());
    }

    #[test]
    fn contains_return_treats_a_dict_set_as_not_a_return() {
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];
        assert!(!contains_return(&body));
    }

    #[test]
    fn block_always_returns_treats_a_dict_set_as_not_always_returning() {
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];
        assert!(!block_always_returns(&body));
    }

    fn a_list_comp_assign_stmt() -> HirStmt {
        HirStmt::ListCompAssign {
            target: "xs".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        }
    }

    fn a_set_comp_assign_stmt() -> HirStmt {
        HirStmt::SetCompAssign {
            target: "xs".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        }
    }

    fn a_dict_comp_assign_stmt() -> HirStmt {
        HirStmt::DictCompAssign {
            target: "xs".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        }
    }

    #[test]
    fn local_name_collection_includes_a_list_comp_assign_s_target_and_synthesized_var() {
        // PR-12 Task 3 (D-117): unlike `DictSet` above, a comprehension
        // introduces two brand-new local names (`target` and the
        // synthesized `var`), not zero.
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![a_list_comp_assign_stmt()];
        let mut names = function_local_names(&params, &body);
        names.sort_unstable();
        assert_eq!(names, vec!["0comp_11_i", "xs"]);
    }

    #[test]
    fn local_name_collection_skips_a_list_comp_assign_s_target_and_var_when_already_local() {
        // Exercises the `!is_local(...)` guard's "already local" branch: a
        // parameter sharing the comprehension's `target`/`var` names must
        // not be pushed a second time.
        let params: Vec<(String, Ty)> = vec![
            ("xs".to_string(), Ty::Infer),
            ("0comp_11_i".to_string(), Ty::Infer),
        ];
        let body = vec![a_list_comp_assign_stmt()];
        let names = function_local_names(&params, &body);
        assert_eq!(names, vec!["xs", "0comp_11_i"]);
    }

    #[test]
    fn local_name_collection_includes_a_set_comp_assign_s_target_and_var() {
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![a_set_comp_assign_stmt()];
        let mut names = function_local_names(&params, &body);
        names.sort_unstable();
        assert_eq!(names, vec!["0comp_11_i", "xs"]);
    }

    #[test]
    fn local_name_collection_includes_a_dict_comp_assign_s_target_and_var() {
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![a_dict_comp_assign_stmt()];
        let mut names = function_local_names(&params, &body);
        names.sort_unstable();
        assert_eq!(names, vec!["0comp_11_i", "xs"]);
    }

    #[test]
    fn contains_return_treats_a_list_comp_assign_as_not_a_return() {
        assert!(!contains_return(&[a_list_comp_assign_stmt()]));
    }

    #[test]
    fn contains_return_treats_a_set_comp_assign_as_not_a_return() {
        assert!(!contains_return(&[a_set_comp_assign_stmt()]));
    }

    #[test]
    fn contains_return_treats_a_dict_comp_assign_as_not_a_return() {
        assert!(!contains_return(&[a_dict_comp_assign_stmt()]));
    }

    #[test]
    fn block_always_returns_treats_a_list_comp_assign_as_not_always_returning() {
        assert!(!block_always_returns(&[a_list_comp_assign_stmt()]));
    }

    #[test]
    fn block_always_returns_treats_a_set_comp_assign_as_not_always_returning() {
        assert!(!block_always_returns(&[a_set_comp_assign_stmt()]));
    }

    #[test]
    fn block_always_returns_treats_a_dict_comp_assign_as_not_always_returning() {
        assert!(!block_always_returns(&[a_dict_comp_assign_stmt()]));
    }

    #[test]
    fn collect_block_constraints_treats_a_list_comp_assign_target_as_a_no_op() {
        // PR-12 Task 3 (D-117), Step 5: mirrors the dict/set/tuple-literal
        // solver-gap precedent directly -- `collect_block_constraints`'s
        // `ListCompAssign`/`SetCompAssign`/`DictCompAssign` arm registers no
        // term for `target` at all (though it does, unlike a bare no-op,
        // still recurse into `elt`/`cond` -- see
        // `private_helper_parameter_is_inferred_through_a_comprehension_s_elt`
        // below for that half). An unrelated private helper with an
        // unresolved (`Ty::Infer`) signature elsewhere in the same module
        // must not let the solver's own leniency swallow a genuine T0034 --
        // it has to fall through to the real, comprehension-aware check pass
        // (mirrors
        // `a_bad_dict_literal_is_still_rejected_when_the_solver_path_runs_first`).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "0comp_11_i".to_string(),
                    iter: CompIter::Range {
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(HirExpr::StringLiteral("x".to_string())),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0034");
    }

    #[test]
    fn private_helper_parameter_is_inferred_through_a_comprehension_s_elt() {
        // Regression test (pinned-reviewer finding, pre-merge, P1): an
        // earlier version of `collect_block_constraints`'s comprehension arm
        // was a bare no-op -- it never recursed into `iter`/`cond`/`elt`
        // at all, unlike every sibling arm in this function (`ForRange`,
        // `ForList`, `DictSet` above all still recurse into their own
        // sub-expressions even though they don't register a term for their
        // own container). That meant a call expression appearing only
        // inside a comprehension's `elt` never participated in this
        // solver's argument<->parameter unification (see
        // `private_parameter_is_inferred_by_forwarding_into_an_annotated_callee`
        // for the plain, non-comprehension version of this exact mechanism):
        // `_forward`'s unannotated `x` parameter, used only as an argument
        // to the fully-annotated `_sink`, used to spuriously fail with
        // "cannot infer type of parameter `x`; add an annotation" even
        // though `x: int` is the only type consistent with this program.
        // `bind_comp_loop_var` + recursing into `elt` (Step 5's actual fix)
        // makes this resolve correctly and reach the real check pass, which
        // accepts the program (`_sink` returns `int`, satisfying the list
        // comprehension's own `T0034` gate).
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(1),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::Call {
                                callee: "_sink".to_string(),
                                args: vec![HirExpr::Name("x".to_string())],
                            }),
                        },
                        HirStmt::Return(None),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn bind_comp_loop_var_unifies_a_range_comprehension_s_loop_variable_with_an_existing_term() {
        // Exercises `bind_comp_loop_var`'s `unify_terms(existing, Ok(Ty::Int), ...)`
        // branch (mirrors `ForRange`'s own analogous branch, exercised by
        // `collect_block_constraints_unifies_a_for_range_loop_variable_s_existing_term`-
        // style coverage elsewhere in this file): the loop variable is
        // already bound to an inferred term (via forwarding into another
        // annotated helper) before the comprehension re-binds it to `Ty::Int`.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_sink".to_string(),
                            args: vec![HirExpr::Name("x".to_string())],
                        }),
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "x".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(1),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_unifies_a_range_comprehension_s_stop_forwarded_from_an_unannotated_parameter()
     {
        // Exercises `bind_comp_loop_var`'s `CompIter::Range` operand-check
        // branch (mirrors `ForRange`'s own analogous `start`/`stop`/`step`
        // checks): the comprehension's own `stop` operand is itself an
        // unresolved solver term (an unannotated parameter forwarded
        // directly as the range bound), which this arm must still visit and
        // unify against `Ty::Int` -- exactly like a plain `for i in
        // range(n):` loop's own `ForRange` arm already does.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![("n".to_string(), Ty::Infer)],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::Name("n".to_string()),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_rejects_a_range_comprehension_whose_stop_was_already_resolved_to_an_incompatible_type()
     {
        // The failure half of the operand-check branch above: `n` is first
        // forwarded into a `str`-typed parameter (resolving its solver term
        // to `Ty::Str`), then reused as the comprehension's own `stop`
        // operand -- `bind_comp_loop_var`'s `unify_terms(term, Ok(Ty::Int),
        // ...)` call must propagate the resulting conflict rather than
        // silently accepting it.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink_str".to_string(),
                    params: vec![("value".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("n".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_sink_str".to_string(),
                            args: vec![HirExpr::Name("n".to_string())],
                        }),
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::Name("n".to_string()),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_rejects_a_range_comprehension_whose_loop_variable_conflicts_with_an_existing_binding()
     {
        // Exercises `bind_comp_loop_var`'s `CompIter::Range` "existing
        // binding" branch's own *failure* path (mirrors `ForRange`'s own
        // `T0023` branch): the comprehension's loop variable happens, in
        // this hand-built test only, to share a name with the enclosing
        // function's own `str`-typed parameter (real lowering's
        // `synthesize_comp_var_name` never produces such a collision), so
        // `Range`'s `Ty::Int` fact genuinely conflicts with the existing
        // binding.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("0comp_11_i".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn collect_block_constraints_gives_a_name_iterable_comprehension_s_loop_variable_a_fresh_term()
    {
        // Exercises `bind_comp_loop_var`'s `CompIter::Name` branch (mirrors
        // `ForList`'s own analogous branch): this solver doesn't track a
        // list-typed name's element type, so the loop variable just gets an
        // unconstrained fresh term here -- real element-type checking is
        // the subsequent `check_with_signatures` pass's job. Every other
        // solver-path comprehension test above uses `CompIter::Range`.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Name("xs".to_string()),
                            cond: None,
                            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_visits_a_set_comp_assign_s_cond_and_elt() {
        // The combined `HirStmt::ListCompAssign | HirStmt::SetCompAssign`
        // solver arm is only exercised with `ListCompAssign` and a `None`
        // `cond` by the tests above -- this pins the `SetCompAssign` half of
        // that pattern and the `cond.is_some()` branch, both distinct
        // coverage regions from the `ListCompAssign`/`cond: None` case.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::SetCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: Some(Box::new(HirExpr::IntLiteral(1))),
                            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_visits_a_dict_comp_assign_s_cond_key_and_value() {
        // Pins the `HirStmt::DictCompAssign` solver arm (its own separate
        // key/value split, not shared with the list/set arm above), with a
        // `cond` present so its own `cond.is_some()` branch is covered too.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::DictCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: Some(Box::new(HirExpr::IntLiteral(1))),
                            key: Box::new(HirExpr::StringLiteral("k".to_string())),
                            value: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_visits_a_dict_comp_assign_s_key_and_value_with_no_cond() {
        // Companion to the test above: pins the `DictCompAssign` solver
        // arm's `cond.is_none()` branch, which that test's `cond: Some(...)`
        // shape never reaches.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::DictCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            key: Box::new(HirExpr::StringLiteral("k".to_string())),
                            value: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_range_comprehension_s_stop_operand() {
        // Pins `bind_comp_loop_var`'s own `collect_expr_constraints(...)?`
        // call for a `CompIter::Range` operand failing outright, distinct
        // from the operand-resolves-but-conflicts tests above. Uses a
        // forward reference to a name assigned *later* in the same body
        // (mirrors `private_helper_inference_rejects_a_read_before_local_assignment`)
        // rather than a plain undefined name: this solver's own
        // `collect_expr_constraints` is deliberately lenient about a
        // non-local undefined name (`None => Ok(None)`, no error -- real
        // "not defined" checking is `infer_expr_in`'s job in the second
        // pass), so only a genuine *local-but-not-yet-bound* read actually
        // triggers this solver's own `unbound_local` error.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::Name("later".to_string()),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(3),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_keeps_a_name_iterable_comprehension_s_loop_variable_s_existing_term()
     {
        // Companion to the "fresh term" test above: pins the
        // `CompIter::Name` branch's `contains_key(var)` guard's *true*
        // branch (variable already bound), mirroring
        // `collect_block_constraints_keeps_a_for_list_loop_variable_s_existing_term`'s
        // own analogous `ForList` coverage. The loop variable is given the
        // same name as a real parameter (`0comp_11_i`), whose own term is
        // seeded into the solver's environment before any statement in the
        // body runs -- unlike a plain `ExprStmt` reference to an
        // undeclared name (which this solver's own local-name tracking
        // would instead reject as unbound, never reaching the
        // comprehension at all).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![
                    ("0comp_11_i".to_string(), Ty::List(Box::new(Ty::Int))),
                    ("ys".to_string(), Ty::List(Box::new(Ty::Int))),
                ],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Name("ys".to_string()),
                        cond: None,
                        elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        // The real check pass still rejects this program overall (`0comp_11_i`
        // is a `list[int]`-typed parameter, and the comprehension tries to
        // rebind it to `int`) -- that is not what this test pins. What
        // matters is that the *solver's* own `bind_comp_loop_var` reaches
        // its `CompIter::Name` branch with `var` already a key in
        // `env.bindings` (from the parameter seeding) and takes the
        // "already bound, do nothing" path without itself erroring, which
        // is what lets the module-wide solver pass complete and fall
        // through to the real, comprehension-aware check pass that then
        // reports the actual `T0023` conflict.
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_list_comp_assign_s_cond() {
        // See the range-stop-operand test above for why a forward reference
        // to a same-body local, not a plain undefined name, is required to
        // exercise this solver's own error path.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: Some(Box::new(HirExpr::Name("later".to_string()))),
                        elt: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_list_comp_assign_s_elt() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::Name("later".to_string())),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_loop_variable_binding()
     {
        // Pins `DictCompAssign`'s own `bind_comp_loop_var(...)?` call site
        // (textually distinct from the list/set arm's own call site above)
        // failing outright, using the same loop-variable-collision shape as
        // the list-comprehension version above.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("0comp_11_i".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::DictCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            key: Box::new(HirExpr::StringLiteral("k".to_string())),
                            value: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_cond() {
        // See `collect_block_constraints_propagates_an_error_from_a_range_comprehension_s_stop_operand`
        // above for why a forward reference to a same-body local, not a
        // plain undefined name, is required to exercise this solver's own
        // error path (rather than being leniently ignored as `Ok(None)`).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::DictCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: Some(Box::new(HirExpr::Name("later".to_string()))),
                        key: Box::new(HirExpr::StringLiteral("k".to_string())),
                        value: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_key() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::DictCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        key: Box::new(HirExpr::Name("later".to_string())),
                        value: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::StringLiteral("k".to_string()),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_value() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::DictCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        key: Box::new(HirExpr::StringLiteral("k".to_string())),
                        value: Box::new(HirExpr::Name("later".to_string())),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_set_item_with_matching_value_type_checks_cleanly() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("b".to_string()),
                value: HirExpr::IntLiteral(2),
            },
        )
        .unwrap();
        // `d[k] = v` never rebinds `x`'s own environment type, unlike an
        // ordinary `Assign`.
        assert_eq!(
            env.lookup("x"),
            Some(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
    }

    #[test]
    fn a_dict_set_item_accepts_a_bool_value_since_bool_is_an_int_subtype() {
        // Mirrors `appending_a_bool_to_a_list_of_int_is_accepted_since_bool_is_an_int_subtype`
        // (D-086): `d[k] = True` is ordinary, CPython-valid Python.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::BoolLiteral(true),
            },
        )
        .unwrap();
    }

    #[test]
    fn a_dict_set_item_with_wrong_key_type_is_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::IntLiteral(1),
                value: HirExpr::IntLiteral(2),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "dict key type mismatch: expected `str`, found `int`"
        );
    }

    #[test]
    fn a_dict_set_item_with_wrong_value_type_is_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::StringLiteral("oops".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "cannot assign `str` to a dict value of `int`");
    }

    #[test]
    fn a_dict_set_item_on_a_non_dict_value_is_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`int` does not support item assignment");
    }

    #[test]
    fn a_dict_set_item_on_a_list_value_is_still_t0033() {
        // D-123 supersedes D-105's "no subscript assignment target anywhere
        // in this file" HIR-level invariant (see `pycc_hir`'s own
        // `subscript_assignment_to_a_bare_name_base_lowers_to_dict_set`
        // test), but `list[int]` itself is still read-only-indexed -- this
        // is where that invariant is actually enforced now.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::IntLiteral(0),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`list[int]` does not support item assignment");
    }

    #[test]
    fn a_dict_set_item_over_an_undefined_name_is_rejected_as_not_defined() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("is not defined"));
    }

    #[test]
    fn a_dict_set_item_propagates_an_ill_typed_key_expression_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::Name("undefined".to_string()),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_dict_set_item_propagates_an_ill_typed_value_expression_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::Name("undefined".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn function_scope_dict_set_item_with_matching_value_type_checks_cleanly() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap();
    }

    #[test]
    fn function_scope_dict_set_item_over_an_unbound_local_is_unbound_local() {
        // Mirrors the local-vs-global unbound distinction `lookup_bound_name`
        // already gives every other function-scope name lookup (e.g.
        // `ForList`'s own `list` field) -- a declared-but-unassigned local
        // reports `unbound_local`, not "not defined".
        let mut env = Environment::new();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("is not bound before this use"));
    }

    // -- PR-11 Task 7 (D-123): set[int] type-checking ---------------------

    #[test]
    fn an_empty_set_literal_cannot_be_inferred() {
        // Unlike `DictLiteral(vec![])`, which is reachable from real source
        // (`{}` parses as an empty dict), an empty `SetLiteral` can only
        // ever be hand-built HIR -- Python has no empty-set literal spelling
        // at all.
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::SetLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty set literal"));
    }

    #[test]
    fn a_homogeneous_set_literal_infers_set_of_int() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::IntLiteral(2),
            HirExpr::IntLiteral(3),
        ]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Set(Box::new(Ty::Int))));
    }

    #[test]
    fn a_set_literal_with_mismatched_element_types_is_rejected_as_t0037() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::StringLiteral("oops".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0037");
        assert_eq!(
            err.message,
            "set element type mismatch: expected int (from the first element), found str"
        );
    }

    #[test]
    fn a_homogeneous_non_int_set_literal_is_rejected_as_t0038() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::StringLiteral("b".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0038");
        assert_eq!(
            err.message,
            "set[str] is not compiled yet (D-122) -- only set[int] is"
        );
        assert!(err.message.contains("set[str]"));
    }

    #[test]
    fn a_bad_set_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The set-shaped counterpart to
        // `a_bad_dict_literal_is_still_rejected_when_the_solver_path_runs_first`:
        // `collect_expr_constraints`'s own `SetLiteral` arm always returns
        // `Ok(None)` (it has no unification-friendly representation for
        // `Ty::Set` either), so an unrelated private helper with an
        // unresolved (`Ty::Infer`) signature must not let the solver's own
        // leniency swallow a genuine T0038 -- it has to fall through to the
        // real, set-aware check pass.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::SetLiteral(vec![
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::StringLiteral("b".to_string()),
                    ]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0038");
    }

    #[test]
    fn a_set_literal_propagates_an_ill_typed_first_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![HirExpr::Name("undefined".to_string())]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_literal_propagates_an_ill_typed_later_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::Name("undefined".to_string()),
        ]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn indexing_a_set_reports_t0033() {
        // Mirrors real CPython: sets are not subscriptable either (D-123) --
        // no explicit `Ty::Set` arm exists in `Subscript`'s own match, and
        // the generic `other` fallthrough already covers it.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`set[int]` does not support indexing");
    }

    #[test]
    fn len_of_a_set_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn constraint_collection_len_call_returns_int_for_a_concretely_bound_set() {
        // Mirrors `constraint_collection_len_call_returns_int_for_a_concretely_bound_dict`
        // above, proving the solver's own relaxed `len()` arm (PR-11 Task 7)
        // also accepts a concretely-bound `Ty::Set` term.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("s".to_string(), Ok(Ty::Set(Box::new(Ty::Int))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("s".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_treats_a_set_literal_as_unconstrained_but_recurses_into_elements() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_set_literal_element() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::SetLiteral(vec![HirExpr::Name("missing".to_string())]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    // -- PR-11b Task 3 (D-116): tuple[...] construction + indexing --------

    #[test]
    fn a_tuple_literal_of_int_bool_float_infers_ty_tuple() {
        let env = Environment::new();
        let expr = HirExpr::TupleLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::BoolLiteral(true),
            HirExpr::FloatLiteral(2.5),
        ]);
        assert_eq!(
            infer_expr(&env, &expr),
            Ok(Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float])))
        );
    }

    #[test]
    fn an_empty_tuple_literal_is_rejected_as_t0021() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::TupleLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty tuple literal"));
    }

    #[test]
    fn a_tuple_literal_with_a_string_element_is_rejected_as_t0039() {
        let env = Environment::new();
        let expr = HirExpr::TupleLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::StringLiteral("a".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0039");
        assert_eq!(
            err.message,
            "tuple element type `str` is not compiled yet (D-116) -- only int/bool/float elements are"
        );
    }

    #[test]
    fn a_tuple_literal_propagates_an_ill_typed_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::TupleLiteral(vec![HirExpr::Name("undefined".to_string())]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn tuple_index_with_a_literal_in_range_int_infers_the_positional_element_type() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "t".to_string(),
                    value: HirExpr::TupleLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::BoolLiteral(true),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("t".to_string())),
                        index: Box::new(HirExpr::IntLiteral(1)),
                    },
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn subscripting_a_tuple_with_a_literal_index_infers_the_positional_element_type() {
        let mut env = Environment::new();
        env.bind(
            "t".to_string(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float])),
        );
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn tuple_index_with_a_non_literal_expression_is_rejected_as_t0040() {
        let mut env = Environment::new();
        env.bind(
            "t".to_string(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])),
        );
        env.bind("i".to_string(), Ty::Int);
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::Name("i".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0040");
        assert_eq!(
            err.message,
            "tuple index must be a non-negative literal integer within range"
        );
    }

    #[test]
    fn tuple_index_out_of_range_is_rejected_as_t0040() {
        let mut env = Environment::new();
        env.bind(
            "t".to_string(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])),
        );
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::IntLiteral(5)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0040");
    }

    #[test]
    fn a_negative_literal_tuple_index_is_rejected_as_t0040() {
        let mut env = Environment::new();
        env.bind(
            "t".to_string(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])),
        );
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::IntLiteral(-1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0040");
    }

    #[test]
    fn a_bad_tuple_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The tuple-shaped counterpart to
        // `a_bad_set_literal_is_still_rejected_when_the_solver_path_runs_first`:
        // `collect_expr_constraints`'s own `TupleLiteral` arm always returns
        // `Ok(None)` (it has no unification-friendly representation for
        // `Ty::Tuple` either), so an unrelated private helper with an
        // unresolved (`Ty::Infer`) signature must not let the solver's own
        // leniency swallow a genuine T0039 -- it has to fall through to the
        // real, tuple-aware check pass.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::TupleLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0039");
    }

    #[test]
    fn constraint_collection_treats_a_tuple_literal_as_unconstrained_but_recurses_into_elements() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_tuple_literal_element() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let expr = HirExpr::TupleLiteral(vec![HirExpr::Name("missing".to_string())]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_top_level_binary_addition_type_checks() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            }))],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_top_level_reference_to_an_undefined_name_is_a_clean_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name(
                "undefined".to_string(),
            )))],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_top_level_call_to_a_previously_defined_function_type_checks() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "main".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_call_a_sibling_function_defined_before_it() {
        // Regression test for D-040: `check_function`'s own env used to be
        // seeded empty, so `main` couldn't see `helper` even though both are
        // ordinary module-level functions -- a valid, non-recursive call
        // between two sibling functions was wrongly rejected with T0021.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![HirExpr::IntLiteral(5)],
                        }],
                    })],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_call_a_sibling_function_defined_after_it() {
        // Same gap as above, but exercising the pre-registration pass (D-039)
        // from the *other* direction: `main` is checked first (it's first in
        // the module) yet still must see `helper`, which is defined later.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![HirExpr::IntLiteral(5)],
                        }],
                    })],
                },
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_read_a_module_level_global_defined_before_it() {
        // Regression test for D-041: reading a module global from a function
        // body needs no `global` declaration in real Python (that's only
        // required to *rebind* one) -- child_for_function used to reset
        // bindings to empty, so `f`'s body couldn't see `x` even though it's
        // an ordinary module-level constant, not some caller's local.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_read_a_module_level_global_defined_after_it() {
        // Same gap, other direction: a function is only ever *called* after
        // the module has (typically) finished running top to bottom, so a
        // global defined later in the file is still visible inside an
        // earlier function's body.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_parameter_shadows_a_module_level_global_of_the_same_name() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("global".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        // If the global (Ty::Str) leaked through instead of the parameter
        // (Ty::Int), this would fail with a T0022 return-type mismatch.
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_later_local_assignment_blocks_fallback_to_a_same_named_global() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn a_read_before_local_assignment_is_local_even_without_a_global() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(2),
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn local_name_collection_deduplicates_assignment_and_for_targets() {
        let body = vec![
            HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(2),
            },
            HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
            HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        ];

        assert_eq!(function_local_names(&[], &body), vec!["x", "i"]);
    }

    #[test]
    fn local_name_collection_deduplicates_for_list_targets() {
        let body = vec![
            HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![],
            },
            HirStmt::ForList {
                var: "i".to_string(),
                list: "ys".to_string(),
                body: vec![],
            },
        ];

        assert_eq!(function_local_names(&[], &body), vec!["i"]);
    }

    #[test]
    fn local_name_collection_deduplicates_annotated_assignment_targets() {
        let body = vec![
            HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
            },
            HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        ];

        assert_eq!(function_local_names(&[], &body), vec!["x"]);
    }

    #[test]
    fn contains_return_treats_an_annotated_assignment_as_not_a_return() {
        let body = vec![HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(1)),
        }];
        assert!(!contains_return(&body));
    }

    #[test]
    fn block_always_returns_treats_an_annotated_assignment_as_not_a_return() {
        let body = vec![HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: None,
        }];
        assert!(!block_always_returns(&body));
    }

    #[test]
    fn contains_return_finds_a_return_inside_a_for_list_loop_body() {
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Return(Some(HirExpr::Name("i".to_string())))],
        }];
        assert!(contains_return(&body));
    }

    #[test]
    fn block_always_returns_treats_a_for_list_loop_as_not_always_returning() {
        // A `for` loop's body might never execute (an empty list), so it can
        // never on its own guarantee a function returns -- same treatment as
        // `ForRange` gets in this same match.
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Return(Some(HirExpr::Name("i".to_string())))],
        }];
        assert!(!block_always_returns(&body));
    }

    #[test]
    fn a_call_before_local_assignment_cannot_fall_back_to_a_global_function() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![],
                        }),
                        HirStmt::Assign {
                            target: "helper".to_string(),
                            value: HirExpr::IntLiteral(1),
                        },
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "local name `helper` is not bound before this use"
        );
    }

    #[test]
    fn a_call_before_local_assignment_cannot_fall_back_to_print() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![],
                    }),
                    HirStmt::Assign {
                        target: "print".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "local name `print` is not bound before this use"
        );
    }

    #[test]
    fn a_bound_local_value_cannot_fall_back_to_a_function_registry_entry() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::Assign {
                            target: "helper".to_string(),
                            value: HirExpr::IntLiteral(1),
                        },
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![],
                        }),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    #[test]
    fn a_parameter_cannot_fall_back_to_a_same_named_builtin() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("print".to_string(), Ty::Int)],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![],
                })],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `print` is bound to a non-callable value");
    }

    #[test]
    fn direct_function_check_uses_the_same_lexical_local_classification() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                },
                HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
            ],
        };

        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn direct_function_check_treats_an_unbound_call_target_as_a_local_read() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                }),
                HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
            ],
        };

        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "local name `helper` is not bound before this use"
        );
    }

    #[test]
    fn direct_function_check_rejects_calling_a_bound_local_value() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                }),
            ],
        };

        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// #133: a module-level value binding must shadow a same-named user
    /// function at every later call site (CPython raises `TypeError: 'int'
    /// object is not callable`; the pre-fix checker resolved the call
    /// through the function registry and accepted it).
    #[test]
    fn a_module_value_binding_shadows_a_same_named_function_call() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// #133: the same shadowing rule covers builtins -- `print = 1` makes a
    /// later `print()` a call on an `int`, which CPython rejects at runtime.
    #[test]
    fn a_module_value_binding_shadows_a_builtin_call() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "print".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `print` is bound to a non-callable value");
    }

    /// #133: pass 3 checks annotated function bodies against the final
    /// module environment (D-041), so a body call whose target is
    /// module-bound to a value is rejected through `infer_expr_in`'s gate.
    #[test]
    fn an_annotated_body_call_sees_the_module_value_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::Function {
                    name: "caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// #133: the solver path (an underscore private helper with an inferred
    /// signature) seeds its body environment from the module globals, so
    /// `collect_expr_constraints`'s mirrored gate rejects the same shape.
    /// Mirrors the issue's own third reproduction end to end through
    /// `check_and_resolve`.
    #[test]
    fn a_private_helper_call_sees_the_module_value_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::Function {
                    name: "_call_helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// D-110's first recorded ordering consequence, pinned as deliberate: a
    /// body call is rejected whenever the callee is value-bound anywhere at
    /// module top level, even when the program's only call executes *before*
    /// the rebinding in CPython's dynamic order (this exact module runs fine
    /// under CPython: `caller()` returns 1, then `helper = 2` rebinds). Pass
    /// 3 checks bodies against the final module environment (D-041), and
    /// D-110 accepts the stricter-than-one-dynamic-trace rejection rather
    /// than importing #22's execution-order model.
    #[test]
    fn a_rebinding_after_the_only_call_still_rejects_the_body_call() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::Function {
                    name: "caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::Call {
                        callee: "caller".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// D-110's second recorded ordering consequence: the binding gate is
    /// callee-first, so a shadowed callee with an additionally-invalid
    /// argument reports the non-callable callee, not the argument's own
    /// error -- uniform with the pre-existing local gate, though CPython's
    /// evaluation order would surface the argument's `NameError` first.
    #[test]
    fn a_shadowed_callee_reports_before_its_argument_errors() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![HirExpr::Name("undefined_name".to_string())],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// D-110's third recorded consequence: a bound value that shadows
    /// nothing at all now reports the accurate non-callable diagnostic
    /// instead of final validation's misleading "call to undefined function".
    #[test]
    fn calling_a_bound_value_that_shadows_nothing_reports_non_callable() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "x".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `x` is bound to a non-callable value");
    }

    /// The discriminating case for `collect_expr_constraints`' mirror gate
    /// (D-110): the bound callee is neither `print` nor any `def`, inside an
    /// inference-signature helper. Without the mirror, `signatures.get`
    /// misses, the call stays unresolved, and the solver dead-ends in
    /// "cannot infer return type of private helper `_h`; add an annotation"
    /// *before* pass 3 ever runs -- a misleading message, since no
    /// annotation fixes calling an `int`. This is the one shape where the
    /// mirror's presence changes the observable diagnostic; a shadowed
    /// `print` would instead be resolved by the special case and caught by
    /// pass 3's own gate later (see the mirror-gate comment).
    #[test]
    fn a_private_helper_calling_a_shadowed_non_function_needs_the_mirror_gate() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "x".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `x` is bound to a non-callable value");
    }

    /// Behavior pin for the shadowed-builtin shape inside an
    /// inference-signature helper: the mirror gate fires before the `print`
    /// special case, so the accurate non-callable diagnostic wins. (Pass 3
    /// would independently reject this shape even without the mirror; the
    /// discriminating case for the mirror itself is the test above.)
    #[test]
    fn a_private_helper_calling_a_shadowed_builtin_is_rejected() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "print".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `print` is bound to a non-callable value");
    }

    /// The round-5 blocker's pin (PR #252's review): a *compatible-type*
    /// value reassignment after the `def` must re-shadow it, even though
    /// `check_assignment`'s already-bound branch never calls `bind()` --
    /// clearing the def-rebound flag only inside `bind()` left it
    /// permanently stuck for any name with a pre-`def` binding, so this
    /// exact module resolved the function where CPython raises `TypeError`.
    #[test]
    fn a_compatible_reassignment_after_the_def_shadows_it_again() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// The round-6 pin (PR #252's review): a private helper's *parameter*
    /// that collides with a module-level def-rebound name re-binds it for
    /// that body, so calling the parameter reports the accurate
    /// non-callable diagnostic -- before the fix the per-function env setup
    /// stripped the name from `bindings` but left the stale def-rebound
    /// flag, skipping the mirror gate and mislabeling a bound parameter as
    /// "not bound before this use".
    #[test]
    fn a_parameter_colliding_with_a_def_rebound_name_reports_non_callable() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("helper".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// The solver-path twin of the test above, proving the two environment
    /// walks agree: the solver's Assign arm already cleared its def-rebound
    /// flag unconditionally, and after the round-5 fix the `Environment`
    /// walk does too.
    #[test]
    fn a_compatible_reassignment_after_the_def_shadows_it_in_the_solver_too() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// The round-4 blocker's pin (PR #252's review): the `def`-rebinding
    /// fact must NOT erase the representation record, so D-040's sticky
    /// rule still rejects `helper = 1; def helper(): ...; helper = "leaked"`
    /// with T0023 -- a version that cleared `bindings` at the `def` let this
    /// program reach codegen with an `int`-allocated slot stored as `str`.
    #[test]
    fn a_value_reassignment_after_the_def_still_enforces_representation() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::StringLiteral("leaked".to_string()),
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(
            err.message,
            "cannot assign `str` to `helper`, previously inferred as `int`"
        );
    }

    /// D-110's source-order `def` rebinding (PR #252's review caught the
    /// initial gate rejecting this): a later `def` rebinds the name over an
    /// earlier value binding, so the call resolves the function exactly
    /// as CPython does (`helper = 1; def helper(): ...; helper()` prints
    /// the function's result there).
    #[test]
    fn a_later_def_rebinds_over_an_earlier_value_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    /// The same source-order `def` rebinding seen from a function body:
    /// pass 3's final module environment reflects the net binding, so a
    /// body call to a name whose value binding was cleared by a later
    /// `def` resolves the function.
    #[test]
    fn a_body_call_resolves_when_a_later_def_rebinds_the_value() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::Function {
                    name: "caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    /// And from the solver path: the accumulated globals clear a value
    /// binding when the same-named `def` follows it, so an
    /// inference-signature helper's body resolves the function.
    #[test]
    fn a_private_helper_resolves_when_a_later_def_rebinds_the_value() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check_and_resolve(&hir).is_ok());
    }

    /// #133's deliberate boundary with #22: pass 2 checks top-level code in
    /// source order, so a call *before* the shadowing assignment still
    /// resolves the function -- the binding simply does not exist yet at
    /// that point. Only the already-active binding shadows.
    #[test]
    fn a_top_level_call_before_the_shadowing_assignment_still_resolves() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn infer_expr_in_rejects_call_before_def_in_top_level() {
        // Directly exercises the call-before-def check in infer_expr_in's
        // HirExpr::Call arm for top-level code (not inside a function body).
        let mut env = Environment::new();
        // Manually insert into functions without adding to defined_functions,
        // simulating a function that exists but hasn't been "executed" yet
        // in source order.
        Arc::make_mut(&mut env.functions).insert(
            "foo".to_string(),
            (vec![], Ty::None),
        );
        // Do NOT insert "foo" into defined_functions -- simulates a call
        // before the def's source-order position.
        let expr = HirExpr::Call {
            callee: "foo".to_string(),
            args: vec![],
        };
        let err = infer_expr_in(&env, &[], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot call function `foo` before its definition"));
    }

    #[test]
    fn a_local_first_assignment_does_not_inherit_the_globals_type() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("global".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_self_referential_first_local_assignment_cannot_read_the_global() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::BinOp {
                                op: BinOpKind::Add,
                                left: Box::new(HirExpr::Name("x".to_string())),
                                right: Box::new(HirExpr::IntLiteral(1)),
                            },
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn an_assignment_nested_in_if_classifies_the_name_as_function_local() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                        HirStmt::If {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::Assign {
                                target: "x".to_string(),
                                value: HirExpr::IntLiteral(2),
                            }],
                            orelse: vec![],
                        },
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn a_nested_for_target_classifies_the_name_as_function_local() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "i".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("i".to_string())),
                        HirStmt::While {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::ForRange {
                                var: "i".to_string(),
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(1),
                                step: HirExpr::IntLiteral(1),
                                body: vec![],
                            }],
                        },
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.message, "local name `i` is not bound before this use");
    }

    #[test]
    fn a_for_target_is_local_while_its_range_operands_are_evaluated() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "i".to_string(),
                    value: HirExpr::IntLiteral(3),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::Name("i".to_string()),
                        step: HirExpr::IntLiteral(1),
                        body: vec![],
                    }],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `i` is not bound before this use");
    }

    #[test]
    fn private_helper_inference_rejects_a_read_before_local_assignment() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "_f".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn check_function_the_public_api_still_has_no_sibling_visibility() {
        // `check_function` is a standalone entry point with no module
        // context, so it must keep working exactly as before: it only ever
        // sees its own signature (needed for recursion), never a sibling's.
        let function = HirItem::Function {
            name: "main".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "helper".to_string(),
                args: vec![],
            })],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_function_body_is_now_checked() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_bare_call_infers_none() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "print".to_string(),
            args: vec![],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn calling_an_undefined_function_is_a_clean_error() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "undefined".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined"));
    }

    #[test]
    fn calling_a_defined_function_infers_its_declared_return_type() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_wide_call_uses_the_heap_argument_type_buffer() {
        let mut env = Environment::new();
        env.bind_function("wide".to_string(), vec![Ty::Int; 5], Ty::Int);
        let expr = HirExpr::Call {
            callee: "wide".to_string(),
            args: vec![HirExpr::IntLiteral(1); 5],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn calling_a_function_with_a_bool_argument_for_an_int_parameter_succeeds() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int], Ty::None);
        let expr = HirExpr::Call {
            callee: "f".to_string(),
            args: vec![HirExpr::BoolLiteral(true)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn calling_a_function_with_the_wrong_number_of_arguments_is_a_clean_error() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("expects 2 argument"));
    }

    #[test]
    fn an_int_argument_for_a_float_parameter_is_a_clean_error() {
        // D-086: pycc requires an explicit `float(...)` conversion at typed
        // boundaries (parameters, returns, assignments) rather than
        // following the Python typing spec's numeric-tower rule that `int`
        // is accepted wherever `float` is annotated. This is a deliberate
        // deviation from `mypy --strict` (which accepts this call), not an
        // oversight -- see D-086's rationale in docs/DECISIONS.md.
        let mut env = Environment::new();
        env.bind_function("identity".to_string(), vec![Ty::Float], Ty::Float);
        let expr = HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message
                .contains("argument 1 of `identity` expects `float`, got `int`")
        );
    }

    #[test]
    fn calling_a_function_with_a_wrong_typed_argument_is_a_clean_error() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::FloatLiteral(2.5)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("argument 2")
                && err.message.contains("int")
                && err.message.contains("float")
        );
    }

    #[test]
    fn a_wide_calls_later_inference_error_precedes_an_earlier_type_mismatch() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int; 5], Ty::None);
        let expr = HirExpr::Call {
            callee: "f".to_string(),
            args: vec![
                HirExpr::StringLiteral("wrong".to_string()),
                HirExpr::IntLiteral(2),
                HirExpr::IntLiteral(3),
                HirExpr::IntLiteral(4),
                HirExpr::Name("undefined".to_string()),
            ],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `undefined` is not defined");
    }

    #[test]
    fn calling_a_function_with_an_undefined_argument_propagates_the_error() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int], Ty::None);
        let expr = HirExpr::Call {
            callee: "f".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_function_s_body_is_checked_against_its_declared_param_types() {
        let function = HirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_return_with_no_value_when_none_is_expected_succeeds() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_return_with_no_value_when_a_value_is_expected_is_a_clean_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
    }

    #[test]
    fn a_return_type_mismatch_is_a_clean_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
    }

    #[test]
    fn a_value_returning_function_must_return_on_every_path() {
        let function = HirItem::Function {
            name: "answer".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            })],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
        assert!(err.message.contains("can exit without returning"));
    }

    #[test]
    fn an_if_with_returns_in_both_branches_satisfies_the_return_contract() {
        let function = HirItem::Function {
            name: "choose".to_string(),
            params: vec![("condition".to_string(), Ty::Bool)],
            return_ty: Ty::Int,
            body: vec![HirStmt::If {
                test: HirExpr::Name("condition".to_string()),
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                orelse: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn an_if_with_only_one_returning_branch_is_t0022() {
        let function = HirItem::Function {
            name: "choose".to_string(),
            params: vec![("condition".to_string(), Ty::Bool)],
            return_ty: Ty::Int,
            body: vec![HirStmt::If {
                test: HirExpr::Name("condition".to_string()),
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                orelse: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0022");
    }

    #[test]
    fn a_return_whose_value_is_undefined_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name(
                "undefined".to_string(),
            )))],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn recursion_is_supported_since_the_function_s_own_signature_is_in_scope() {
        let function = HirItem::Function {
            name: "count".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "count".to_string(),
                args: vec![HirExpr::Name("n".to_string())],
            }))],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_function_s_if_while_and_for_bodies_are_checked_against_its_return_type() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::While {
                        test: HirExpr::BoolLiteral(true),
                        body: vec![HirStmt::ForRange {
                            var: "i".to_string(),
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(1),
                            step: HirExpr::IntLiteral(1),
                            body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                        }],
                    }],
                    orelse: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                },
                HirStmt::Return(Some(HirExpr::IntLiteral(2))),
            ],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_bad_return_nested_in_if_while_and_for_is_still_caught() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::While {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(1),
                        step: HirExpr::IntLiteral(1),
                        body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                    }],
                }],
                orelse: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
    }

    #[test]
    #[should_panic(expected = "check_function called with a non-Function HirItem")]
    fn check_function_panics_on_a_non_function_item() {
        let _ = check_function(&HirItem::TopLevelStmt(HirStmt::Return(None)));
    }

    #[test]
    fn an_if_s_test_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::Name("undefined".to_string()),
                body: vec![],
                orelse: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_s_orelse_ill_typed_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_with_assignments_in_a_function_whose_body_has_an_error_propagates_via_clone_path() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::ExprStmt(HirExpr::Name("undefined".to_string())),
                ],
                orelse: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_with_assignments_in_a_function_whose_orelse_has_an_error_propagates_via_clone_path() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_with_assignments_in_a_function_whose_body_has_an_error_propagates_via_clone_path() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::ExprStmt(HirExpr::Name("undefined".to_string())),
                ],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_s_test_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::While {
                test: HirExpr::Name("undefined".to_string()),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_start_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::Name("undefined".to_string()),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_stop_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::Name("undefined".to_string()),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_step_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::Name("undefined".to_string()),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_list_loop_binds_its_variable_as_the_list_element_type_in_a_function_body() {
        // Direct `check_stmt_in_function` call (matching this file's own
        // established pattern, e.g. the `AnnAssign` tests just above) rather
        // than `check_function`: a for-loop body might never execute (an
        // empty list), so `block_always_returns` always treats it as "does
        // not always return" (same as `ForRange`) -- a function whose only
        // statement is a `for` loop can never satisfy a non-`None` return
        // type, regardless of what's inside the loop body.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("i".to_string()),
            }],
        };
        check_stmt_in_function(&mut env, &["xs", "i", "y"], &stmt, Ty::None).unwrap();
        // Issue #118 Part 1: loop variable and body-only binding are Maybe.
        assert_eq!(env.lookup("i"), None);
        assert_eq!(env.lookup("y"), None);
        assert_eq!(env.lookup_any("i"), Some(Ty::Int));
        assert_eq!(env.lookup_any("y"), Some(Ty::Int));
    }

    #[test]
    fn a_for_dict_loop_binds_its_variable_as_the_key_type_in_a_function_body() {
        // Mirrors the list-shaped test above (PR-11 Task 3, D-123): `for k in
        // d:` binds `k` as the dict's key type, not its value type.
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let stmt = HirStmt::ForList {
            var: "k".to_string(),
            list: "d".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("k".to_string()),
            }],
        };
        check_stmt_in_function(&mut env, &["d", "k", "y"], &stmt, Ty::None).unwrap();
        // Issue #118 Part 1: loop variable and body-only binding are Maybe.
        assert_eq!(env.lookup("k"), None);
        assert_eq!(env.lookup("y"), None);
        assert_eq!(env.lookup_any("k"), Some(Ty::Str));
        assert_eq!(env.lookup_any("y"), Some(Ty::Str));
    }

    #[test]
    fn a_for_set_loop_binds_its_variable_as_the_element_type_in_a_function_body() {
        // Mirrors the dict-shaped test above (PR-11 Task 7, D-123): `for x in
        // s:` binds `x` as the set's element type.
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let stmt = HirStmt::ForList {
            var: "x".to_string(),
            list: "s".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("x".to_string()),
            }],
        };
        check_stmt_in_function(&mut env, &["s", "x", "y"], &stmt, Ty::None).unwrap();
        // Issue #118 Part 1: loop variable and body-only binding are Maybe.
        assert_eq!(env.lookup("x"), None);
        assert_eq!(env.lookup("y"), None);
        assert_eq!(env.lookup_any("x"), Some(Ty::Int));
        assert_eq!(env.lookup_any("y"), Some(Ty::Int));
    }

    #[test]
    fn a_function_local_if_with_no_else_then_read_is_t0041() {
        // `def f(): if cond: x = 1; return x` -- T0041 (possibly-unbound
        // read in function body).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("cond".to_string(), Ty::Bool)],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::Name("cond".to_string()),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                        orelse: vec![],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_function_local_while_then_read_is_t0041() {
        // `def f(): while cond: x = 1; return x` -- T0041.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("cond".to_string(), Ty::Bool)],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::While {
                        test: HirExpr::Name("cond".to_string()),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_function_local_both_branches_if_then_read_succeeds() {
        // `def f(): if cond: x = 1 else: x = 2; return x` -- succeeds.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("cond".to_string(), Ty::Bool)],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::Name("cond".to_string()),
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
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_local_for_range_then_read_is_t0041() {
        // `def f(): for i in range(3): x = i; return x` -- T0041 (the loop
        // body may execute zero times, so `x` is maybe-bound).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::Name("i".to_string()),
                        }],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_function_local_for_range_var_then_read_is_t0041() {
        // `def f(): for i in range(3): pass; return i` -- T0041 (the loop
        // variable is maybe-bound after the loop).
        let hir = HirModule {
            items: vec![HirItem::Function {
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
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_function_local_pre_bound_then_for_range_var_read_succeeds() {
        // `def f(): i = 0; for i in range(3): pass; return i` -- succeeds
        // because `i` was definitely bound before the loop.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::Assign {
                        target: "i".to_string(),
                        value: HirExpr::IntLiteral(0),
                    },
                    HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                        body: vec![],
                    },
                    HirStmt::Return(Some(HirExpr::Name("i".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_local_for_list_then_read_is_t0041() {
        // `def f(xs: list[int]): for i in xs: x = i; return x` -- T0041.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::ForList {
                        var: "i".to_string(),
                        list: "xs".to_string(),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::Name("i".to_string()),
                        }],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn a_function_local_pre_bound_for_list_var_survives_as_definite() {
        // `def f(xs: list[int]): i = 0; for i in xs: pass; return i` --
        // succeeds because `i` was definitely bound before the loop
        // (issue #118 Part 1).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::Assign {
                        target: "i".to_string(),
                        value: HirExpr::IntLiteral(0),
                    },
                    HirStmt::ForList {
                        var: "i".to_string(),
                        list: "xs".to_string(),
                        body: vec![],
                    },
                    HirStmt::Return(Some(HirExpr::Name("i".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_local_maybe_then_definite_upgrade_succeeds() {
        // `def f(c: bool): if c: x = 1; x = 2; return x` -- the maybe-bound
        // `x` from the if-body is upgraded to definite by the unconditional
        // `x = 2`, so the return read succeeds.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("c".to_string(), Ty::Bool)],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::Name("c".to_string()),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                        orelse: vec![],
                    },
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(2),
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_for_list_s_list_not_defined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "undefined".to_string(),
                body: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn a_for_list_s_list_read_before_a_later_local_assignment_is_unbound_local() {
        // `xs` is a declared local of this function (it's assigned further
        // down), so reading it in the `for` clause before that assignment
        // must be `unbound_local`, not "not defined" -- the same
        // is_local-aware distinction `HirExpr::Name` itself makes.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ForList {
                    var: "i".to_string(),
                    list: "xs".to_string(),
                    body: vec![],
                },
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
            ],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn a_for_list_loop_over_a_non_list_value_in_a_function_body_is_rejected() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "x".to_string(),
                body: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_for_list_loop_whose_body_statement_is_ill_typed_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_list_loop_rejects_a_conflicting_reassignment_of_its_variable_in_a_function_body() {
        // Same as the module-scope sibling above: `check_assignment` itself
        // is what rejects rebinding an already-`str` parameter to the list's
        // `int` element type.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("i".to_string(), Ty::Str),
                ("xs".to_string(), Ty::List(Box::new(Ty::Int))),
            ],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    // -- PR-12 Task 3 (D-117): comprehension type-checking, function scope --

    #[test]
    fn a_list_comprehension_over_range_type_checks_in_a_function_body() {
        // Also pins that `var` is bound before `elt` is checked at function
        // scope (mirrors the module-scope sibling above): if the ordering
        // were wrong, `elt`'s reference to the loop variable would fail as
        // an unbound local.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_list_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_list_comprehension_producing_a_non_int_element_is_rejected_as_t0034_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::StringLiteral("x".to_string())),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[str]"));
    }

    #[test]
    fn a_set_comprehension_over_a_bare_set_parameter_type_checks_in_a_function_body() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Set` at function scope.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("s".to_string(), Ty::Set(Box::new(Ty::Int)))],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_20_x".to_string(),
                iter: CompIter::Name("s".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_20_x".to_string())),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_set_comprehension_producing_a_non_int_element_is_rejected_as_t0038_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::StringLiteral("x".to_string())),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0038");
        assert!(err.message.contains("set[str]"));
    }

    #[test]
    fn a_set_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_dict_comprehension_over_a_bare_dict_parameter_type_checks_in_a_function_body() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Dict` at function scope, binding `var` as the dict's key
        // type.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))))],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_20_k".to_string(),
                iter: CompIter::Name("d".to_string()),
                cond: None,
                key: Box::new(HirExpr::Name("0comp_20_k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_dict_comprehension_producing_a_non_str_int_pair_is_rejected_as_t0036_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::IntLiteral(1)),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0036");
        assert!(err.message.contains("dict[int, int]"));
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_comprehension_over_a_forward_referenced_iterable_is_rejected_as_unbound_local() {
        // Mirrors `a_for_list_loop_over_a_forward_referenced_list...`-style
        // behavior: `xs` is a genuine function-local name (assigned later in
        // the same body), so referencing it before that assignment is
        // `unbound_local`, not `not defined`.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ListCompAssign {
                    target: "y".to_string(),
                    var: "0comp_11_i".to_string(),
                    iter: CompIter::Name("xs".to_string()),
                    cond: None,
                    elt: Box::new(HirExpr::IntLiteral(1)),
                },
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
            ],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    // Per-arm error-propagation pins (function scope): the same set as the
    // module-scope block above, `check_stmt_in_function`'s own three arms.

    #[test]
    fn a_set_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_dict_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_list_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("0comp_11_i".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_set_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("0comp_11_i".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_dict_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("0comp_11_i".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_list_comprehension_s_if_filter_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_if_filter_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_list_comprehension_s_elt_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("undefined".to_string())),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_elt_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("undefined".to_string())),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_key_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::Name("undefined".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_value_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::Name("undefined".to_string())),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_identity_signature_is_inferred_from_its_call_site_and_return() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn check_and_resolve_takes_the_fast_path_for_an_already_concrete_valid_module() {
        // Pre-existing gap noticed while chasing this task's own 100%
        // coverage requirement: every other `check_and_resolve` test in this
        // file uses a `Ty::Infer` signature, so `checked_function_signatures`'s
        // fast-path *success* return (`concrete_function_signatures` is
        // `Some` and `check_with_signatures` succeeds) was never exercised.
        // Unrelated to this task's list-typing work, but cheap and in-scope
        // to close here rather than leave dangling.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "identity".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(resolved, hir);
    }

    #[test]
    fn check_and_resolve_materializes_private_signatures_without_mutating_input() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_identity".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }
        );
        assert_eq!(
            hir.items[0],
            HirItem::Function {
                name: "_identity".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }
        );
    }

    #[test]
    fn annotated_int_result_propagates_back_to_a_private_binary_parameter() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("value".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("value".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            }
        );
    }

    #[test]
    fn annotated_int_result_propagates_back_to_a_right_binary_parameter() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::Name("value".to_string())),
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::Name("value".to_string())),
                }))],
            }
        );
    }

    #[test]
    fn annotated_int_result_rejects_a_known_string_left_operand() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::StringLiteral("wrong".to_string())),
                    right: Box::new(HirExpr::Name("value".to_string())),
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn annotated_int_result_rejects_a_known_string_right_operand() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("value".to_string())),
                    right: Box::new(HirExpr::StringLiteral("wrong".to_string())),
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_parameter_is_inferred_by_forwarding_into_an_annotated_callee() {
        // Regression test (self-review finding, pre-merge): the solver used
        // to only unify a call argument against a callee's parameter when
        // the callee's own parameter term was itself unresolved. When the
        // callee is fully annotated (its parameter term is already
        // `Ok(Ty::Int)`), an unresolved *caller* argument variable never got
        // constrained in that direction, even though `unify_terms` itself
        // already supports it symmetrically -- so `_forward` below used to
        // fail with a spurious "add an annotation" T0021 instead of
        // correctly inferring `x: int` from forwarding into `_sink`.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "_sink".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    })],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_binary_helper_signature_is_inferred_across_operator_constraints() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_add".to_string(),
                    params: vec![
                        ("left".to_string(), Ty::Infer),
                        ("right".to_string(), Ty::Infer),
                    ],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("left".to_string())),
                        right: Box::new(HirExpr::Name("right".to_string())),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_add".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_true_division_helper_infers_a_float_return() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_ratio".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Div,
                        left: Box::new(HirExpr::Name("value".to_string())),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ratio".to_string(),
                    value: HirExpr::Call {
                        callee: "_ratio".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    },
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_helper_without_a_return_infers_none() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_log".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::FString(vec![
                        FStringPart::Literal("equal=".to_string()),
                        FStringPart::Interpolation(Box::new(HirExpr::Compare {
                            op: CmpOpKind::Eq,
                            left: Box::new(HirExpr::IntLiteral(1)),
                            right: Box::new(HirExpr::IntLiteral(1)),
                        })),
                    ])],
                })],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_helper_with_a_bare_return_infers_none() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_stop".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(None)],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_constant_helper_infers_a_float_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_constant".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::FloatLiteral(1.5)))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_range_helper_infers_its_parameter_as_int() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_loop".to_string(),
                params: vec![("limit".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Name("limit".to_string()),
                    step: HirExpr::IntLiteral(1),
                    body: vec![HirStmt::While {
                        test: HirExpr::BoolLiteral(false),
                        body: vec![HirStmt::If {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                                callee: "print".to_string(),
                                args: vec![HirExpr::Name("item".to_string())],
                            })],
                            orelse: vec![],
                        }],
                    }],
                }],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        check(&hir).unwrap();
    }

    #[test]
    fn unresolved_private_parameter_requests_an_annotation() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_constant".to_string(),
                params: vec![("unused".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("parameter `unused`"));
    }

    #[test]
    fn unresolved_private_return_requests_an_annotation() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_unknown".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("missing".to_string())))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("return type"));
    }

    #[test]
    fn undefined_call_cannot_silently_resolve_a_private_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_unknown".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "missing".to_string(),
                    args: vec![],
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn unresolved_binary_operand_cannot_silently_resolve_a_private_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_unknown".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("missing".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn unresolved_private_binary_parameters_request_annotations() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_add".to_string(),
                params: vec![
                    ("left".to_string(), Ty::Infer),
                    ("right".to_string(), Ty::Infer),
                ],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("left".to_string())),
                    right: Box::new(HirExpr::Name("right".to_string())),
                }))],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn unresolved_call_argument_does_not_invent_a_private_parameter_type() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::Name("missing".to_string())],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_parameter_inference_rejects_conflicting_call_sites() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::StringLiteral("one".to_string())],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_return_inference_rejects_conflicting_return_types() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_choose".to_string(),
                params: vec![("condition".to_string(), Ty::Bool)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::If {
                    test: HirExpr::Name("condition".to_string()),
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                    orelse: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
                        "one".to_string(),
                    )))],
                }],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0022");
    }

    fn nested_private_call_conflict() -> HirExpr {
        HirExpr::Call {
            callee: "_sink".to_string(),
            args: vec![HirExpr::Call {
                callee: "_identity".to_string(),
                args: vec![HirExpr::StringLiteral("wrong".to_string())],
            }],
        }
    }

    fn private_constraint_error_fixture(stmt: HirStmt) -> HirModule {
        HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
                HirItem::Function {
                    name: "_probe".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![stmt],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        }
    }

    #[test]
    fn private_f_string_and_return_propagate_nested_constraint_errors() {
        let stmt = HirStmt::Return(Some(HirExpr::FString(vec![FStringPart::Interpolation(
            Box::new(nested_private_call_conflict()),
        )])));
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_slice_base_wrapping_a_conflict_is_rejected_end_to_end() {
        // PR-12 Task 7 (D-118). NOTE what this test does and does NOT pin:
        // it proves `check()`'s real, end-to-end pipeline still rejects a
        // nested private-helper call conflict when it sits inside a
        // `Slice`'s `base`, *inside a private helper's own body* (not just
        // at module top level, which the pair above this whole group
        // already covers) -- a real and valuable thing to test on its own.
        // It does NOT, on its own, prove that `collect_expr_constraints`'s
        // `Slice` arm itself recurses into `base`: `check_with_environment`'s
        // Pass 3 independently re-type-checks every function body via
        // `infer_expr_in` regardless of what the solver's own
        // constraint-collection pass did or didn't visit, so this exact
        // conflict is still caught by Pass 3 even with the solver arm's
        // recursion mutated away to a bare `Ok(None)` no-op (confirmed by
        // temporarily making that mutation and rerunning this test: it kept
        // passing). `private_slice_base_recursion_pins_an_otherwise_unconstrained_parameter`
        // below is the test that actually is sensitive to the solver arm's
        // own recursion, using a construction where Pass 3 cannot mask a
        // missing visit (a `Ty::Infer` parameter whose *only* constraining
        // use anywhere in the module is inside a `Slice`'s `base`, so
        // signature resolution itself -- which gates whether Pass 3 ever
        // runs at all -- depends on the solver reaching it).
        let stmt = HirStmt::Return(Some(HirExpr::Slice {
            base: Box::new(nested_private_call_conflict()),
            start: None,
            stop: None,
            step: None,
        }));
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_slice_bound_wrapping_a_conflict_is_rejected_end_to_end() {
        // Same as above, but the conflict is nested inside a bound
        // (`start`), not `base`. Same caveat applies: this alone doesn't
        // pin the solver arm's own recursion (Pass 3 masks it identically),
        // see `private_slice_start_bound_recursion_pins_an_otherwise_unconstrained_parameter`
        // below for the test that actually does.
        let stmt = HirStmt::Return(Some(HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: Some(Box::new(nested_private_call_conflict())),
            stop: None,
            step: None,
        }));
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_slice_base_recursion_pins_an_otherwise_unconstrained_parameter() {
        // Load-bearing counterpart to the pair above (added after review:
        // those two tests' original doc comments overclaimed what they
        // pinned -- Pass 3 independently re-type-checks every function body
        // regardless of solver recursion, so a genuine conflict is caught
        // either way; verified by temporarily gutting the `Slice` arm in
        // `collect_expr_constraints` to a bare `Ok(None)` and confirming
        // those two tests still passed).
        //
        // This test's construction is different in a way that matters:
        // `_forward`'s own parameter `xs` is `Ty::Infer`, and its *only*
        // appearance anywhere in this module is wrapped in a call
        // (`_id_list(xs)`) embedded in a `Slice`'s `base`. Nothing else ever
        // constrains `xs`'s type. `collect_expr_constraints`'s `Call` arm is
        // the one place that performs real unification (`unify_terms`)
        // between an argument's term and the callee's own (here: concretely
        // annotated) parameter term -- so `xs`'s term becomes concretely
        // `list[int]` if and only if the solver actually visits
        // `_id_list(xs)`, which only happens if the outer `Slice` arm
        // recurses into `base`.
        //
        // If that recursion is missing, `xs`'s term never resolves, and
        // `infer_function_signatures_with_solver`'s own final resolution
        // loop raises "cannot infer type of parameter `xs`... add an
        // annotation" *before `check_with_signatures`/Pass 3 ever runs for
        // this function* -- Pass 3 cannot mask a failure that happens
        // before Pass 3 is even reached. Confirmed empirically: gutting the
        // `Slice` arm to a bare `Ok(None)` flips this test from `Ok(())` to
        // exactly that `Err`; restoring the real arm makes it pass again.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_id_list".to_string(),
                    params: vec![("v".to_string(), Ty::List(Box::new(Ty::Int)))],
                    return_ty: Ty::List(Box::new(Ty::Int)),
                    body: vec![HirStmt::Return(Some(HirExpr::Name("v".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("xs".to_string(), Ty::Infer)],
                    return_ty: Ty::List(Box::new(Ty::Int)),
                    body: vec![HirStmt::Return(Some(HirExpr::Slice {
                        base: Box::new(HirExpr::Call {
                            callee: "_id_list".to_string(),
                            args: vec![HirExpr::Name("xs".to_string())],
                        }),
                        start: Some(Box::new(HirExpr::IntLiteral(1))),
                        stop: None,
                        step: None,
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn private_slice_start_bound_recursion_pins_an_otherwise_unconstrained_parameter() {
        // Same load-bearing shape as the test above, but the embedded call
        // sits in a bound (`start`) rather than `base`: `_forward`'s own
        // parameter `x` is `Ty::Infer` and is used *only* wrapped in
        // `_id(x)` inside the slice's `start` bound. A bare `Name("x")`
        // bound would NOT be load-bearing here -- looking up a name in
        // `collect_expr_constraints` performs no unification of its own,
        // only the `Call` arm does, which is why this test wraps `x` in a
        // call rather than mirroring the brief-review's own bare `xs[x:]`
        // sketch literally.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_id".to_string(),
                    params: vec![("v".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("v".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::List(Box::new(Ty::Int)),
                    body: vec![HirStmt::Return(Some(HirExpr::Slice {
                        base: Box::new(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
                        start: Some(Box::new(HirExpr::Call {
                            callee: "_id".to_string(),
                            args: vec![HirExpr::Name("x".to_string())],
                        })),
                        stop: None,
                        step: None,
                    }))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn private_if_body_propagates_nested_constraint_errors() {
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(nested_private_call_conflict())],
            orelse: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_while_body_propagates_nested_constraint_errors() {
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(nested_private_call_conflict())],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_range_operand_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ForRange {
            var: "item".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: nested_private_call_conflict(),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_range_body_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ForRange {
            var: "item".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(1),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(nested_private_call_conflict())],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_compare_left_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(nested_private_call_conflict()),
            right: Box::new(HirExpr::IntLiteral(1)),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_compare_right_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(nested_private_call_conflict()),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_binary_left_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(nested_private_call_conflict()),
            right: Box::new(HirExpr::IntLiteral(1)),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_binary_right_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(nested_private_call_conflict()),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_assignment_propagates_nested_constraint_errors() {
        let stmt = HirStmt::Assign {
            target: "value".to_string(),
            value: nested_private_call_conflict(),
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_assignment_with_an_unresolved_value_is_checked_after_inference() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_assign".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Assign {
                    target: "value".to_string(),
                    value: HirExpr::Name("missing".to_string()),
                }],
            }],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_if_test_propagates_nested_constraint_errors() {
        let stmt = HirStmt::If {
            test: nested_private_call_conflict(),
            body: vec![],
            orelse: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_while_test_propagates_nested_constraint_errors() {
        let stmt = HirStmt::While {
            test: nested_private_call_conflict(),
            body: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_range_parameter_rejects_a_conflicting_call_site_type() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_loop".to_string(),
                    params: vec![("limit".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::ForRange {
                        var: "item".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::Name("limit".to_string()),
                        step: HirExpr::IntLiteral(1),
                        body: vec![],
                    }],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_loop".to_string(),
                    args: vec![HirExpr::StringLiteral("wrong".to_string())],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_implicit_none_return_rejects_an_int_constrained_call_site() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_noop".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![],
                },
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Call {
                        callee: "_noop".to_string(),
                        args: vec![],
                    },
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0022");
    }

    #[test]
    fn private_division_return_rejects_an_int_constrained_call_site() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_ratio".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Div,
                        left: Box::new(HirExpr::Name("value".to_string())),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Call {
                        callee: "_ratio".to_string(),
                        args: vec![HirExpr::IntLiteral(4)],
                    },
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_binary_constraint_rejects_incompatible_resolved_operands() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_bad_add".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("value".to_string())),
                        right: Box::new(HirExpr::StringLiteral("wrong".to_string())),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_bad_add".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn direct_check_function_rejects_an_unresolved_private_signature() {
        let function = HirItem::Function {
            name: "_identity".to_string(),
            params: vec![("value".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn type_solver_covers_concrete_and_union_merge_paths() {
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        assert!(
            !unify_terms(
                Ok(Ty::Int),
                Ok(Ty::Bool),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );
        assert!(
            unify_terms(
                Ok(Ty::Int),
                Ok(Ty::Str),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .is_err()
        );

        let empty_left = fresh_term(&mut parents, &mut concrete);
        let empty_right = fresh_term(&mut parents, &mut concrete);
        assert!(
            unify_terms(
                empty_left.clone(),
                empty_right.clone(),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );
        assert!(
            !unify_terms(
                empty_left,
                empty_right,
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );

        let typed_left = fresh_term(&mut parents, &mut concrete);
        let typed_right = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            typed_left.clone(),
            Ok(Ty::Bool),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed_right.clone(),
            Ok(Ty::Int),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed_left,
            typed_right.clone(),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        assert_eq!(
            resolved_term(typed_right, &mut parents, &concrete),
            Some(Ty::Int)
        );

        let typed = fresh_term(&mut parents, &mut concrete);
        let empty = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            typed.clone(),
            Ok(Ty::Str),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed,
            empty.clone(),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        assert_eq!(resolved_term(empty, &mut parents, &concrete), Some(Ty::Str));

        let conflicting_left = fresh_term(&mut parents, &mut concrete);
        let conflicting_right = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            conflicting_left.clone(),
            Ok(Ty::Int),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            conflicting_right.clone(),
            Ok(Ty::Str),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        assert!(
            unify_terms(
                conflicting_left,
                conflicting_right,
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .is_err()
        );

        let reversed = fresh_term(&mut parents, &mut concrete);
        assert!(
            unify_terms(
                Ok(Ty::Float),
                reversed.clone(),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );
        assert_eq!(
            resolved_term(reversed, &mut parents, &concrete),
            Some(Ty::Float)
        );
    }

    fn generic_identity_fn(param_ty: Ty, return_ty: Ty) -> HirItem {
        HirItem::Function {
            name: "identity".to_string(),
            params: vec![("x".to_string(), param_ty)],
            return_ty,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        }
    }

    #[test]
    fn check_generic_function_accepts_consistent_single_type_param() {
        let param = Ty::Param(Box::new("T".to_string()));
        let func = generic_identity_fn(param.clone(), param);
        assert!(check_generic_function(&func).is_ok());
    }

    #[test]
    fn check_generic_function_rejects_scalar_specific_op_on_bare_param() {
        // `def f[T](x: T) -> T: return x + 1` -- `T` is opaque, so `+` on a
        // bare `Ty::Param` value must fail exactly like any other
        // type-incompatible operand pair, reusing the existing `T0021`
        // numeric-operator diagnostic (`numeric_result_type`'s own
        // "operator ... not defined for ..." arm), not a new code.
        let param = Ty::Param(Box::new("T".to_string()));
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), param.clone())],
            return_ty: param,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("x".to_string())),
                right: Box::new(HirExpr::IntLiteral(1)),
            }))],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn check_generic_function_rejects_two_distinct_type_parameters() {
        // Defense in depth (`crates/pycc_hir`'s own frontend arity gate
        // already prevents this from real source, Task 1): a
        // hand-constructed `HirItem` with two distinct `Ty::Param` names
        // across its signature must still be rejected here.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("x".to_string(), Ty::Param(Box::new("T".to_string()))),
                ("y".to_string(), Ty::Param(Box::new("U".to_string()))),
            ],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn check_generic_function_rejects_container_position_type_parameter() {
        // Defense in depth, same rationale as the two-type-parameter test
        // above: `crates/pycc_hir`'s `annotation_to_ty` never lowers a
        // `list[T]`-shaped annotation from real source at all, so this can
        // only be exercised via a hand-built `HirItem`.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "xs".to_string(),
                Ty::List(Box::new(Ty::Param(Box::new("T".to_string())))),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn instantiate_generic_call_substitutes_int_and_mangles_name() {
        let param = Ty::Param(Box::new("T".to_string()));
        let func = generic_identity_fn(param.clone(), param);
        let instantiation = instantiate_generic_call(&func, &[Ty::Int]).unwrap();
        assert_eq!(instantiation.mangled_name, "0gen_identity__T_int");
        assert_eq!(instantiation.return_ty, Ty::Int);
        assert_eq!(
            instantiation.specialized,
            HirItem::Function {
                name: "0gen_identity__T_int".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }
        );
    }

    #[test]
    fn instantiate_generic_call_substitutes_str_at_a_different_call_site() {
        // Two call sites resolving `T` to different concrete types must
        // produce two independent, differently-mangled specializations
        // (D-134's own monomorphization requirement) -- and the same
        // concrete type from two call sites must produce the identical
        // mangled name and body, which is what Task 3 dedupes on.
        let param = Ty::Param(Box::new("T".to_string()));
        let func = generic_identity_fn(param.clone(), param);
        let str_instantiation = instantiate_generic_call(&func, &[Ty::Str]).unwrap();
        assert_eq!(str_instantiation.mangled_name, "0gen_identity__T_str");
        assert_eq!(str_instantiation.return_ty, Ty::Str);

        let int_instantiation_1 = instantiate_generic_call(&func, &[Ty::Int]).unwrap();
        let int_instantiation_2 = instantiate_generic_call(&func, &[Ty::Int]).unwrap();
        assert_eq!(
            int_instantiation_1.mangled_name,
            int_instantiation_2.mangled_name
        );
        assert_eq!(int_instantiation_1, int_instantiation_2);
    }

    #[test]
    fn instantiate_generic_call_substitutes_nested_ann_assign_annotation() {
        // Exercises the recursive body-substitution walk itself: a nested
        // `AnnAssign` inside an `If` body is the one HIR shape that carries
        // an embedded `Ty` beyond the function's own signature, and must be
        // substituted too, not just `params`/`return_ty`. Compares the whole
        // specialized item by value (rather than destructuring it with a
        // `let-else`/`unreachable!()`, which this file's own coverage gate
        // would otherwise always flag as an untaken branch).
        let param = Ty::Param(Box::new("T".to_string()));
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), param.clone())],
            return_ty: param.clone(),
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::AnnAssign {
                    target: "y".to_string(),
                    annotation: param,
                    value: None,
                }],
                orelse: vec![],
            }],
        };
        let instantiation = instantiate_generic_call(&func, &[Ty::Bool]).unwrap();
        assert_eq!(
            instantiation.specialized,
            HirItem::Function {
                name: "0gen_f__T_bool".to_string(),
                params: vec![("x".to_string(), Ty::Bool)],
                return_ty: Ty::Bool,
                body: vec![HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::AnnAssign {
                        target: "y".to_string(),
                        annotation: Ty::Bool,
                        value: None,
                    }],
                    orelse: vec![],
                }],
            }
        );
    }

    #[test]
    fn instantiate_generic_call_substitutes_nested_while_forrange_forlist_bodies() {
        // Exercises the remaining recursive-body-walk arms (`While`,
        // `ForRange`, `ForList`) that the `If` test above doesn't reach --
        // each must recurse into its own nested `body` to find and
        // substitute a further-nested `AnnAssign`.
        let param = Ty::Param(Box::new("T".to_string()));
        let nested = |marker: &str| {
            vec![HirStmt::AnnAssign {
                target: marker.to_string(),
                annotation: Ty::Param(Box::new("T".to_string())),
                value: None,
            }]
        };
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), param.clone())],
            return_ty: param,
            body: vec![
                HirStmt::While {
                    test: HirExpr::BoolLiteral(true),
                    body: nested("w"),
                },
                HirStmt::ForRange {
                    var: "i".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(1),
                    step: HirExpr::IntLiteral(1),
                    body: nested("r"),
                },
                HirStmt::ForList {
                    var: "e".to_string(),
                    list: "xs".to_string(),
                    body: nested("l"),
                },
            ],
        };
        let instantiation = instantiate_generic_call(&func, &[Ty::Int]).unwrap();
        let expected_nested = |marker: &str| {
            vec![HirStmt::AnnAssign {
                target: marker.to_string(),
                annotation: Ty::Int,
                value: None,
            }]
        };
        assert_eq!(
            instantiation.specialized,
            HirItem::Function {
                name: "0gen_f__T_int".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::While {
                        test: HirExpr::BoolLiteral(true),
                        body: expected_nested("w"),
                    },
                    HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(1),
                        step: HirExpr::IntLiteral(1),
                        body: expected_nested("r"),
                    },
                    HirStmt::ForList {
                        var: "e".to_string(),
                        list: "xs".to_string(),
                        body: expected_nested("l"),
                    },
                ],
            }
        );
    }

    #[test]
    #[should_panic(expected = "instantiate_generic_call called with a non-Function HirItem")]
    fn instantiate_generic_call_panics_on_a_non_function_item() {
        let _ = instantiate_generic_call(&HirItem::TopLevelStmt(HirStmt::Return(None)), &[]);
    }

    #[test]
    #[should_panic(expected = "check_function called with a non-Function HirItem")]
    fn check_generic_function_skips_the_shape_gate_for_a_top_level_statement() {
        // `check_generic_function`'s own shape gate only applies to a
        // `HirItem::Function` -- a `TopLevelStmt` item has no signature to
        // scan for a type parameter at all, so it skips straight to
        // `check_function`, which still panics on a non-`Function` item
        // exactly as it always has (see `check_function_panics_on_a_non_function_item`
        // above): this test's own job is only to prove the `if let` shape
        // gate itself doesn't reject a `TopLevelStmt` some other way first.
        let item = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::IntLiteral(1)));
        let _ = check_generic_function(&item);
    }

    #[test]
    fn check_generic_function_rejects_type_parameter_nested_in_a_dict_key() {
        // Defense in depth: a hand-built `dict[T, str]`-shaped parameter,
        // covering `scan_signature_ty_for_param`'s `Ty::Dict` arm on its
        // key position (the `?` on the key's own recursive call).
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "d".to_string(),
                Ty::Dict(Box::new((Ty::Param(Box::new("T".to_string())), Ty::Str))),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn check_generic_function_rejects_type_parameter_nested_in_a_dict_value() {
        // Covers the dict value position too (the key succeeds, so this
        // exercises the `Ty::Dict` arm's second recursive call instead of
        // its first).
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "d".to_string(),
                Ty::Dict(Box::new((Ty::Str, Ty::Param(Box::new("T".to_string()))))),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn check_generic_function_rejects_type_parameter_nested_in_a_set() {
        // Defense in depth: covers `scan_signature_ty_for_param`'s
        // `Ty::List(elem) | Ty::Set(elem)` arm's `Set` alternative
        // specifically (the `List` case is already covered by
        // `check_generic_function_rejects_container_position_type_parameter`
        // above).
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "xs".to_string(),
                Ty::Set(Box::new(Ty::Param(Box::new("T".to_string())))),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn check_generic_function_rejects_container_position_type_parameter_in_the_return_type() {
        // Every other container-position test above puts the offending
        // shape in a parameter; `generic_type_param_name` scans
        // `return_ty` too (via its own separate call), which needs its own
        // test to cover that specific call site's error path.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::List(Box::new(Ty::Param(Box::new("T".to_string())))),
            body: vec![HirStmt::Return(None)],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn check_generic_function_rejects_type_parameter_nested_in_a_tuple_element() {
        // Defense in depth: covers `scan_signature_ty_for_param`'s
        // `Ty::Tuple` arm.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "t".to_string(),
                Ty::Tuple(Box::new(vec![
                    Ty::Int,
                    Ty::Param(Box::new("T".to_string())),
                ])),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_generic_function(&func).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn check_generic_function_accepts_a_non_generic_tuple_typed_parameter() {
        // `check_generic_function` scans every function's signature, not
        // just an actually-generic one -- a plain, fully concrete
        // `Ty::Tuple` parameter with no `Ty::Param` anywhere inside it must
        // scan clean, exercising `scan_signature_ty_for_param`'s `Ty::Tuple`
        // arm's own success path (its `for` loop finishing without any
        // element raising `T0042`).
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Str])))],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        assert!(check_generic_function(&func).is_ok());
    }

    #[test]
    fn instantiate_generic_call_leaves_a_concrete_sibling_parameter_unchanged() {
        // A generic function can mix a `T`-typed parameter with an
        // ordinary concrete-typed one (see the assignability-rejection
        // test above for the failure path); when the call site's argument
        // for that concrete parameter is compatible, `substitute_ty` must
        // still run on it and simply clone it through unchanged, since it
        // carries no `Ty::Param` occurrence to substitute.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("x".to_string(), Ty::Param(Box::new("T".to_string()))),
                ("n".to_string(), Ty::Int),
            ],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let instantiation = instantiate_generic_call(&func, &[Ty::Str, Ty::Int]).unwrap();
        assert_eq!(
            instantiation.specialized,
            HirItem::Function {
                name: "0gen_f__T_str".to_string(),
                params: vec![("x".to_string(), Ty::Str), ("n".to_string(), Ty::Int),],
                return_ty: Ty::Str,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }
        );
    }

    #[test]
    fn instantiate_generic_call_rejects_inconsistent_call_site_substitution() {
        // `def f[T](x: T, y: T) -> T` called as `f(1, "a")` -- both
        // occurrences of `T` must agree (D-134's own named example).
        let param = Ty::Param(Box::new("T".to_string()));
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("x".to_string(), param.clone()),
                ("y".to_string(), param.clone()),
            ],
            return_ty: param,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let err = instantiate_generic_call(&func, &[Ty::Int, Ty::Str]).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn instantiate_generic_call_rejects_non_scalar_call_site_argument() {
        // A call site whose argument type used to resolve `T` is not one
        // of the four scalars is D-134's own named rejection case (e.g.
        // passing a `list[int]` value where `T` is inferred).
        let param = Ty::Param(Box::new("T".to_string()));
        let func = generic_identity_fn(param.clone(), param);
        let err = instantiate_generic_call(&func, &[Ty::List(Box::new(Ty::Int))]).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn instantiate_generic_call_rejects_wrong_arity() {
        let param = Ty::Param(Box::new("T".to_string()));
        let func = generic_identity_fn(param.clone(), param);
        let err = instantiate_generic_call(&func, &[Ty::Int, Ty::Int]).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn instantiate_generic_call_rejects_unresolvable_type_parameter() {
        // `T` appears only in the return type, never in any parameter --
        // no call-site argument can resolve it.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
        };
        let err = instantiate_generic_call(&func, &[Ty::Int]).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn instantiate_generic_call_rejects_non_generic_function() {
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let err = instantiate_generic_call(&func, &[Ty::Int]).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn instantiate_generic_call_checks_non_generic_parameter_assignability() {
        // A generic function can mix a `T`-typed parameter with an
        // ordinary concrete-typed one; the concrete one still gets
        // checked for assignability using the existing `is_assignable`
        // (and the existing `T0021` "argument expects" message shape),
        // not silently skipped.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("x".to_string(), Ty::Param(Box::new("T".to_string()))),
                ("n".to_string(), Ty::Int),
            ],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let err = instantiate_generic_call(&func, &[Ty::Int, Ty::Str]).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn instantiate_generic_call_rejects_two_distinct_type_parameters() {
        // Defense in depth, mirrors `check_generic_function`'s own test.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("x".to_string(), Ty::Param(Box::new("T".to_string()))),
                ("y".to_string(), Ty::Param(Box::new("U".to_string()))),
            ],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let err = instantiate_generic_call(&func, &[Ty::Int, Ty::Int]).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    #[test]
    fn instantiate_generic_call_rejects_container_position_type_parameter() {
        // Defense in depth, mirrors `check_generic_function`'s own test.
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "xs".to_string(),
                Ty::List(Box::new(Ty::Param(Box::new("T".to_string())))),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let err = instantiate_generic_call(&func, &[Ty::List(Box::new(Ty::Int))]).unwrap_err();
        assert_eq!(err.code, "T0042");
    }

    // PR-13 Task 3 (D-133/D-134): full pipeline wiring -- `check`/
    // `check_and_resolve` dispatching a generic call site through
    // `instantiate_generic_call`, and `check_and_resolve`'s own
    // `monomorphize` pass rewriting call sites and dropping/appending
    // items so `pycc_mir::build` only ever sees ordinary concrete
    // functions.

    fn find_function<'a>(module: &'a HirModule, name: &str) -> Option<&'a HirItem> {
        module
            .items
            .iter()
            .find(|item| matches!(item, HirItem::Function { name: n, .. } if n == name))
    }

    fn count_function(module: &HirModule, name: &str) -> usize {
        module
            .items
            .iter()
            .filter(|item| matches!(item, HirItem::Function { name: n, .. } if n == name))
            .count()
    }

    #[test]
    fn check_alone_type_checks_a_module_containing_a_valid_generic_function() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let top = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }],
        }));
        let hir = HirModule {
            items: vec![identity, top],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn check_reports_the_same_generic_diagnostic_as_check_and_resolve() {
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("x".to_string(), Ty::Param(Box::new("T".to_string()))),
                ("y".to_string(), Ty::Param(Box::new("U".to_string()))),
            ],
            return_ty: Ty::Param(Box::new("T".to_string())),
            body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
        };
        let hir = HirModule {
            items: vec![func],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0042");
        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0042");
    }

    #[test]
    fn check_and_resolve_rejects_a_generic_call_site_with_the_wrong_arity() {
        // Exercises `infer_expr_in`'s own generic-dispatch arm propagating
        // an `instantiate_generic_call` error (as opposed to
        // `check_generic_function`'s shape gate, covered by the sibling
        // test above) through both public entry points.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let top = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        }));
        let hir = HirModule {
            items: vec![identity.clone(), top.clone()],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
        let hir = HirModule {
            items: vec![identity, top],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn check_and_resolve_rejects_a_type_parameter_nested_in_a_parameter_container() {
        let func = HirItem::Function {
            name: "f".to_string(),
            params: vec![(
                "xs".to_string(),
                Ty::List(Box::new(Ty::Param(Box::new("T".to_string())))),
            )],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        let hir = HirModule {
            items: vec![func],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0042");
    }

    #[test]
    fn ty_contains_param_detects_a_param_nested_in_every_container_shape() {
        let t = Ty::Param(Box::new("T".to_string()));
        assert!(ty_contains_param(&t));
        assert!(ty_contains_param(&Ty::List(Box::new(t.clone()))));
        assert!(ty_contains_param(&Ty::Set(Box::new(t.clone()))));
        assert!(ty_contains_param(&Ty::Dict(Box::new((Ty::Str, t.clone())))));
        assert!(ty_contains_param(&Ty::Dict(Box::new((t.clone(), Ty::Int)))));
        assert!(ty_contains_param(&Ty::Tuple(Box::new(vec![
            Ty::Int,
            t.clone()
        ]))));
        assert!(!ty_contains_param(&Ty::Int));
        assert!(!ty_contains_param(&Ty::Tuple(Box::new(vec![
            Ty::Int,
            Ty::Str
        ]))));
    }

    #[test]
    fn is_generic_signature_detects_a_param_nested_in_a_parameter_container() {
        assert!(is_generic_signature(
            &[(
                "xs".to_string(),
                Ty::List(Box::new(Ty::Param(Box::new("T".to_string()))))
            )],
            &Ty::None,
        ));
        assert!(is_generic_signature(
            &[],
            &Ty::List(Box::new(Ty::Param(Box::new("T".to_string()))))
        ));
        assert!(!is_generic_signature(
            &[("x".to_string(), Ty::Int)],
            &Ty::Int
        ));
    }

    #[test]
    fn check_and_resolve_monomorphizes_a_nested_generic_call_and_drops_the_original() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let hir = HirModule {
            items: vec![
                identity,
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    }],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert!(find_function(&resolved, "identity").is_none());
        assert!(find_function(&resolved, "0gen_identity__T_int").is_some());
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
        // Compared against the whole expected statement (not a
        // `let-else { panic!() }` destructure) so the never-taken failure
        // arm isn't its own uncovered branch -- mirrors this file's
        // existing convention (see Task 2's own note on `unreachable!()` in
        // test bodies).
        assert_eq!(
            resolved.items[0],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "0gen_identity__T_int".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                }],
            }))
        );
    }

    #[test]
    fn monomorphize_lets_a_non_generic_function_read_a_module_global_defined_later_in_the_file_alongside_a_generic_function()
     {
        // Fix-round regression test (finding #1): `check` (via
        // `check_with_environment`'s three-pass D-041 discipline) and
        // `check_and_resolve`/`monomorphize` must agree on validity for a
        // module that mixes a generic function with a non-generic function
        // reading a module-level global assigned *after* that function's
        // own `def` -- Python only evaluates a function body when called,
        // typically after the whole module has already run top to bottom.
        // Before this fix, `monomorphize` walked `hir.items` in a single
        // source-order pass, so `uses_global`'s body was rewritten before
        // the later `g: int = 5` top-level assignment had grown `env`,
        // wrongly reporting `g` as undefined even though `check` (using the
        // correct two-phase order) already accepted the exact same program.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let uses_global = HirItem::Function {
            name: "uses_global".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name("g".to_string())))],
        };
        let global_assign = HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "g".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(5)),
        });
        let call_uses_global = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "uses_global".to_string(),
                args: vec![],
            }],
        }));
        let call_identity = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }],
        }));
        let hir = HirModule {
            items: vec![
                identity,
                uses_global.clone(),
                global_assign,
                call_uses_global,
                call_identity,
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        // `check` accepts this module (three-pass discipline lets
        // `uses_global` see `g` regardless of source position).
        assert!(check(&hir).is_ok());
        // `check_and_resolve`/`monomorphize` must accept it too, and must
        // not have dropped or mis-rewritten the non-generic function.
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(find_function(&resolved, "uses_global"), Some(&uses_global));
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_monomorphizes_two_call_sites_at_different_concrete_types_into_distinct_specializations()
     {
        // Fix-round regression test (finding #3): the actual monomorphization
        // crux is that two call sites at *different* concrete types produce
        // two distinct, correctly-routed specializations -- not the same
        // specialization reused, and not swapped between call sites.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let call_int = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }],
        }));
        let call_str = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::StringLiteral("s".to_string())],
            }],
        }));
        let hir = HirModule {
            items: vec![identity, call_int, call_str],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert!(find_function(&resolved, "identity").is_none());

        let int_specialization = find_function(&resolved, "0gen_identity__T_int")
            .expect("an `int` specialization must exist");
        let str_specialization = find_function(&resolved, "0gen_identity__T_str")
            .expect("a `str` specialization must exist");
        assert_ne!(int_specialization, str_specialization);
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
        assert_eq!(count_function(&resolved, "0gen_identity__T_str"), 1);

        // Each call site's rewritten callee must point to its own
        // specialization -- not the same one, not swapped. The original
        // generic `identity` def (index 0) is dropped entirely, so the two
        // top-level call statements shift down to indices 0 and 1.
        assert_eq!(
            resolved.items[0],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "0gen_identity__T_int".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                }],
            }))
        );
        assert_eq!(
            resolved.items[1],
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Call {
                    callee: "0gen_identity__T_str".to_string(),
                    args: vec![HirExpr::StringLiteral("s".to_string())],
                }],
            }))
        );
    }

    #[test]
    fn check_and_resolve_dedupes_two_call_sites_with_the_same_concrete_type() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let use_twice = HirItem::Function {
            name: "use_twice".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                HirStmt::Assign {
                    target: "a".to_string(),
                    value: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    },
                },
                HirStmt::Assign {
                    target: "b".to_string(),
                    value: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    },
                },
                HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("a".to_string())),
                    right: Box::new(HirExpr::Name("b".to_string())),
                })),
            ],
        };
        let top = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(3)],
            }],
        }));
        let hir = HirModule {
            items: vec![identity, use_twice, top],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_monomorphizes_a_generic_function_alongside_an_inferred_private_helper() {
        // `_helper`'s own `Ty::Infer` signature defeats `concrete_function_environment`/
        // `concrete_function_signatures`'s fast path, forcing the solver-inferred
        // path -- this exercises `check_with_signatures`'s own `bind_generic`
        // registration (the fast path's registration lives in
        // `concrete_function_environment` instead, already covered by every
        // other test in this group).
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        // `_helper`'s call site deliberately does not itself involve a
        // generic call -- the solver path's own constraint collection
        // (`collect_expr_constraints`, a separate implementation from
        // `infer_expr_in`) has no generic-call dispatch of its own, so
        // mixing the two would exercise a distinct, unrelated gap rather
        // than the `bind_generic` registration this test targets.
        let helper = HirItem::Function {
            name: "_helper".to_string(),
            params: vec![("v".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::Name("v".to_string())))],
        };
        let use_helper = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "_helper".to_string(),
            args: vec![HirExpr::IntLiteral(5)],
        }));
        let top = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }],
        }));
        let hir = HirModule {
            items: vec![identity, helper, use_helper, top],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert!(find_function(&resolved, "identity").is_none());
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn an_unannotated_private_helper_fed_a_generic_call_s_result_reports_a_clean_diagnostic_instead_of_leaking_ty_param()
     {
        // Fix-round regression test (finding #2): `_helper(v)` has no
        // annotation, so its parameter type must be inferred from its call
        // site's argument -- `_helper(identity(1))`, where `identity` is a
        // still-generic function whose call this solver-based inference
        // path cannot instantiate (it has no notion of
        // `instantiate_generic_call`). Before this fix, the solver
        // unified `_helper`'s fresh inference variable directly against
        // `identity`'s raw, uninstantiated `Ty::Param("T")` signature,
        // producing a confusing diagnostic that named the internal `T`
        // representation directly (e.g. "operator Add is not defined for
        // `T` and `int`"). This must instead fail with a clean, dedicated
        // diagnostic that never mentions `Ty::Param`/`T` as if it were a
        // real type.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let helper = HirItem::Function {
            name: "_helper".to_string(),
            params: vec![("v".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("v".to_string())),
                right: Box::new(HirExpr::IntLiteral(1)),
            }))],
        };
        let use_helper = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "_helper".to_string(),
            args: vec![HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }],
        }));
        let hir = HirModule {
            items: vec![identity, helper, use_helper],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0042");
        assert!(
            !err.message.contains("Ty::Param") && !err.message.contains("\"T\""),
            "diagnostic must never leak the raw `Ty::Param` internal representation, got: {}",
            err.message
        );
        // `check` (the validation-only entry point) must fail the same way.
        let check_err = check(&hir).unwrap_err();
        assert_eq!(check_err.code, "T0042");
    }

    #[test]
    fn an_unannotated_private_helper_s_argument_passed_directly_into_a_generic_parameter_reports_a_clean_diagnostic()
     {
        // Complements the test above by exercising the *other* operand of
        // `collect_expr_constraints`'s own `Ty::Param`-leak guard: here it
        // is the callee's own uninstantiated parameter type
        // (`identity`'s `x: T`) that is `Ok(Ty::Param(_))`, not the
        // caller-supplied argument -- `_helper(x)`'s own unannotated `x` is
        // the unresolved (`Err`) side instead. Both operands of the `||`
        // need their own case to reach `true` (the other test's argument
        // is already resolved when this one's is not, and vice versa),
        // since Rust's `||` short-circuits and a covering test for only one
        // operand leaves the other's `true` branch unreached.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let helper = HirItem::Function {
            name: "_helper".to_string(),
            params: vec![("x".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            }))],
        };
        let use_helper = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "_helper".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        }));
        let hir = HirModule {
            items: vec![identity, helper, use_helper],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0042");
        assert_eq!(check(&hir).unwrap_err().code, "T0042");
    }

    #[test]
    fn an_unannotated_private_helper_s_return_type_fed_directly_from_a_generic_call_reports_a_clean_diagnostic()
     {
        // Exercises `unify_terms`'s own dedicated `Ty::Param`-leak guard
        // directly (as opposed to `collect_expr_constraints`'s own Call-arm
        // guard, covered by the two tests above): a private helper's
        // *return* type is unified against its body's final expression
        // unconditionally (`collect_block_constraints`'s `Return` handling
        // has no analogous pre-guard of its own), so `_helper`'s
        // unannotated return type unifying directly against `identity(1)`'s
        // own uninstantiated `Ty::Param` return term must still produce a
        // clean `T0042`, not a leaked-`Ty::Param` diagnostic.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let helper = HirItem::Function {
            name: "_helper".to_string(),
            params: vec![],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            }))],
        };
        let use_helper = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "_helper".to_string(),
            args: vec![],
        }));
        let hir = HirModule {
            items: vec![identity, helper, use_helper],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(check_and_resolve(&hir).unwrap_err().code, "T0042");
        assert_eq!(check(&hir).unwrap_err().code, "T0042");
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_binop_and_compare() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                }),
                right: Box::new(HirExpr::IntLiteral(2)),
            }))],
        };
        let g = HirItem::Function {
            name: "g".to_string(),
            params: vec![],
            return_ty: Ty::Bool,
            body: vec![HirStmt::Return(Some(HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                }),
                right: Box::new(HirExpr::IntLiteral(2)),
            }))],
        };
        let hir = HirModule {
            items: vec![identity, f, g],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_a_generic_call_inside_an_fstring_interpolation() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::FString(vec![
                FStringPart::Literal("x=".to_string()),
                FStringPart::Interpolation(Box::new(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ])))],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_list_set_and_tuple_literals() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    }]),
                },
                HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::SetLiteral(vec![HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    }]),
                },
                HirStmt::Assign {
                    target: "zs".to_string(),
                    value: HirExpr::TupleLiteral(vec![
                        HirExpr::Call {
                            callee: "identity".to_string(),
                            args: vec![HirExpr::IntLiteral(3)],
                        },
                        HirExpr::IntLiteral(4),
                    ]),
                },
                HirStmt::Return(None),
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_a_generic_call_inside_a_dict_literal() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    },
                )]),
            }],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_subscript_and_slice() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                },
                HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        index: Box::new(HirExpr::Call {
                            callee: "identity".to_string(),
                            args: vec![HirExpr::IntLiteral(0)],
                        }),
                    },
                },
                HirStmt::Assign {
                    target: "z".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: Some(Box::new(HirExpr::Call {
                            callee: "identity".to_string(),
                            args: vec![HirExpr::IntLiteral(0)],
                        })),
                        stop: Some(Box::new(HirExpr::IntLiteral(2))),
                        step: None,
                    },
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_list_append_and_set_add() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::ExprStmt(HirExpr::ListAppend {
                    list: "xs".to_string(),
                    value: Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    }),
                }),
                HirStmt::ExprStmt(HirExpr::SetAdd {
                    set: "ys".to_string(),
                    value: Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(3)],
                    }),
                }),
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_dict_get_or_default() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                },
                HirStmt::Assign {
                    target: "v".to_string(),
                    value: HirExpr::DictGetOrDefault {
                        dict: "d".to_string(),
                        key: Box::new(HirExpr::Call {
                            callee: "identity".to_string(),
                            args: vec![HirExpr::StringLiteral("a".to_string())],
                        }),
                        default: Box::new(HirExpr::Call {
                            callee: "identity".to_string(),
                            args: vec![HirExpr::IntLiteral(0)],
                        }),
                    },
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_str"), 1);
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_if_while_and_for_range() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::If {
                    test: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::BoolLiteral(true)],
                    },
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                    orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    })],
                },
                HirStmt::While {
                    test: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::BoolLiteral(false)],
                    },
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(3)],
                    })],
                },
                HirStmt::ForRange {
                    var: "i".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(3)],
                    },
                    step: HirExpr::IntLiteral(1),
                    body: vec![HirStmt::ExprStmt(HirExpr::Name("i".to_string()))],
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_bool"), 1);
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_a_generic_call_inside_for_list_over_a_list() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::ForList {
                    var: "x".to_string(),
                    list: "xs".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    })],
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_a_generic_call_inside_for_list_over_a_dict() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                },
                HirStmt::ForList {
                    var: "k".to_string(),
                    list: "d".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::Name("k".to_string())],
                    })],
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_str"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_a_generic_call_inside_for_list_over_a_set() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "s".to_string(),
                    value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::ForList {
                    var: "x".to_string(),
                    list: "s".to_string(),
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    })],
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_ann_assign_with_and_without_a_value() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::AnnAssign {
                    target: "y".to_string(),
                    annotation: Ty::Int,
                    value: None,
                },
                HirStmt::AnnAssign {
                    target: "z".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    }),
                },
                HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::IntLiteral(2),
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_a_dict_set_statement() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                },
                HirStmt::DictSet {
                    dict: "d".to_string(),
                    key: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::StringLiteral("b".to_string())],
                    },
                    value: HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    },
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_str"), 1);
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_rewrites_generic_calls_inside_comprehension_assignments() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "0comp_v".to_string(),
                    iter: CompIter::Range {
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::Call {
                            callee: "identity".to_string(),
                            args: vec![HirExpr::IntLiteral(3)],
                        },
                        step: HirExpr::IntLiteral(1),
                    },
                    cond: Some(Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::BoolLiteral(true)],
                    })),
                    elt: Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::Name("0comp_v".to_string())],
                    }),
                },
                HirStmt::SetCompAssign {
                    target: "ys".to_string(),
                    var: "0comp_w".to_string(),
                    iter: CompIter::Name("xs".to_string()),
                    cond: Some(Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::BoolLiteral(true)],
                    })),
                    elt: Box::new(HirExpr::Name("0comp_w".to_string())),
                },
                HirStmt::DictCompAssign {
                    target: "zs".to_string(),
                    var: "0comp_u".to_string(),
                    iter: CompIter::Name("ys".to_string()),
                    cond: Some(Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::BoolLiteral(false)],
                    })),
                    key: Box::new(HirExpr::StringLiteral("k".to_string())),
                    value: Box::new(HirExpr::Name("0comp_u".to_string())),
                },
                HirStmt::SetCompAssign {
                    target: "ks".to_string(),
                    var: "0comp_p".to_string(),
                    iter: CompIter::Name("zs".to_string()),
                    cond: None,
                    elt: Box::new(HirExpr::IntLiteral(1)),
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
        assert_eq!(count_function(&resolved, "0gen_identity__T_bool"), 1);
    }

    #[test]
    fn check_and_resolve_leaves_a_list_pop_expression_untouched_as_a_structural_leaf() {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::ExprStmt(HirExpr::ListPop {
                    list: "xs".to_string(),
                }),
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(2)],
                }),
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn monomorphize_propagates_an_instantiation_error_from_inside_a_function_body() {
        // Bypasses ordinary validation deliberately, same rationale as the
        // `ForList`/`CompIter` defensive-fallback tests above: a real
        // program with this wrong-arity call would already be rejected by
        // `check`/`check_and_resolve` before `monomorphize` ever ran. This
        // exercises the `?` propagation out of `rewrite_generic_calls_in_stmt`
        // inside `monomorphize`'s own `HirItem::Function` branch.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "identity".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            })],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(monomorphize(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn monomorphize_propagates_an_instantiation_error_from_a_top_level_statement() {
        // Same rationale as the function-body variant above, for
        // `monomorphize`'s `HirItem::TopLevelStmt` branch.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let top = HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        }));
        let hir = HirModule {
            items: vec![identity, top],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(monomorphize(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn monomorphize_propagates_an_instantiation_error_from_a_dict_comp_assign_cond() {
        // Bypasses ordinary validation deliberately, same rationale as the
        // other direct `monomorphize` tests in this group -- exercises the
        // `?` propagation out of `rewrite_generic_calls_in_stmt`'s
        // `DictCompAssign` arm specifically for its `cond` sub-expression.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
                HirStmt::DictCompAssign {
                    target: "zs".to_string(),
                    var: "0comp_v".to_string(),
                    iter: CompIter::Name("xs".to_string()),
                    cond: Some(Box::new(HirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                    })),
                    key: Box::new(HirExpr::StringLiteral("k".to_string())),
                    value: Box::new(HirExpr::IntLiteral(1)),
                },
            ],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(monomorphize(&hir).unwrap_err().code, "T0021");
    }

    /// A generic call that always fails `instantiate_generic_call` with
    /// `T0021` (wrong arity) -- used throughout the error-propagation group
    /// below to prove every structural recursion position in
    /// `rewrite_generic_calls_in_expr`/`rewrite_generic_calls_in_stmt`/
    /// `rewrite_comp_iter` actually propagates a nested failure instead of
    /// silently swallowing it.
    fn bad_generic_call() -> HirExpr {
        HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        }
    }

    fn assert_monomorphize_propagates_error(body: Vec<HirStmt>) {
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body,
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert_eq!(monomorphize(&hir).unwrap_err().code, "T0021");
    }

    // PR-13 Task 3: every structural recursion position in
    // `rewrite_generic_calls_in_expr`/`rewrite_generic_calls_in_stmt`/
    // `rewrite_comp_iter` needs its own covering error-propagation case for
    // the 100%-region D-014 gate -- grouped into a few multi-assertion
    // tests (rather than one `#[test]` per position) since each case is a
    // one-line `bad_generic_call()` substitution, not independent behavior
    // worth its own test name.

    #[test]
    fn rewrite_generic_calls_in_expr_propagates_errors_from_every_recursive_position() {
        // `Call` args (line ~3213) and the `arg_tys` collection step (~3219,
        // via an unresolved argument name rather than `instantiate_generic_call`
        // itself).
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![bad_generic_call()],
        })]);
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        })]);
        // `BinOp`/`Compare`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(bad_generic_call()),
            right: Box::new(HirExpr::IntLiteral(1)),
        })]);
        // `FString`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::FString(vec![
            FStringPart::Interpolation(Box::new(bad_generic_call())),
        ]))]);
        // `ListLiteral`/`SetLiteral`/`TupleLiteral`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::ListLiteral(vec![
            bad_generic_call(),
        ]))]);
        // `DictLiteral`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::DictLiteral(vec![
            (HirExpr::StringLiteral("a".to_string()), bad_generic_call()),
        ]))]);
        // `Subscript`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::Subscript {
            base: Box::new(bad_generic_call()),
            index: Box::new(HirExpr::IntLiteral(0)),
        })]);
        // `Slice`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::Slice {
            base: Box::new(bad_generic_call()),
            start: None,
            stop: None,
            step: None,
        })]);
        // `ListAppend`/`SetAdd`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::ListAppend {
            list: "xs".to_string(),
            value: Box::new(bad_generic_call()),
        })]);
        // `DictGetOrDefault`.
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(bad_generic_call()),
            default: Box::new(HirExpr::IntLiteral(0)),
        })]);
        // `AttrGet`'s own `base` (D-154).
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(bad_generic_call()),
            attr: "x".to_string(),
        })]);
        // `MethodCall`'s own `base` and its per-argument loop (D-154).
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(bad_generic_call()),
            method: "m".to_string(),
            args: vec![],
        })]);
        assert_monomorphize_propagates_error(vec![HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::IntLiteral(0)),
            method: "m".to_string(),
            args: vec![bad_generic_call()],
        })]);
    }

    #[test]
    fn rewrite_generic_calls_in_stmt_propagates_errors_from_every_recursive_position() {
        // `Assign`.
        assert_monomorphize_propagates_error(vec![HirStmt::Assign {
            target: "y".to_string(),
            value: bad_generic_call(),
        }]);
        // `AnnAssign` (`Some(value)`).
        assert_monomorphize_propagates_error(vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(bad_generic_call()),
        }]);
        // `If`'s own `test`, `body`, and `orelse`.
        assert_monomorphize_propagates_error(vec![HirStmt::If {
            test: bad_generic_call(),
            body: vec![],
            orelse: vec![],
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(bad_generic_call())],
            orelse: vec![],
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![],
            orelse: vec![HirStmt::ExprStmt(bad_generic_call())],
        }]);
        // `While`'s own `test` and `body`.
        assert_monomorphize_propagates_error(vec![HirStmt::While {
            test: bad_generic_call(),
            body: vec![],
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(bad_generic_call())],
        }]);
        // `ForRange`'s own bounds and `body`.
        assert_monomorphize_propagates_error(vec![HirStmt::ForRange {
            var: "i".to_string(),
            start: bad_generic_call(),
            stop: HirExpr::IntLiteral(1),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(1),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(bad_generic_call())],
        }]);
        // `ForList`'s own `body`.
        assert_monomorphize_propagates_error(vec![
            HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            },
            HirStmt::ForList {
                var: "x".to_string(),
                list: "xs".to_string(),
                body: vec![HirStmt::ExprStmt(bad_generic_call())],
            },
        ]);
        // `DictSet`'s own `key`/`value`.
        assert_monomorphize_propagates_error(vec![
            HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            },
            HirStmt::DictSet {
                dict: "d".to_string(),
                key: bad_generic_call(),
                value: HirExpr::IntLiteral(1),
            },
        ]);
        // `ListCompAssign`/`SetCompAssign`/`DictCompAssign`'s own
        // `rewrite_comp_iter` propagation (a bad `CompIter::Range` bound)
        // and their own `cond`/`elt`/`key`/`value` propagation.
        assert_monomorphize_propagates_error(vec![HirStmt::ListCompAssign {
            target: "xs".to_string(),
            var: "v".to_string(),
            iter: CompIter::Range {
                start: bad_generic_call(),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::ListCompAssign {
            target: "xs".to_string(),
            var: "v".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(bad_generic_call()),
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::SetCompAssign {
            target: "xs".to_string(),
            var: "v".to_string(),
            iter: CompIter::Range {
                start: bad_generic_call(),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::SetCompAssign {
            target: "xs".to_string(),
            var: "v".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(bad_generic_call()),
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::DictCompAssign {
            target: "xs".to_string(),
            var: "v".to_string(),
            iter: CompIter::Range {
                start: bad_generic_call(),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::DictCompAssign {
            target: "xs".to_string(),
            var: "v".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(bad_generic_call()),
            value: Box::new(HirExpr::IntLiteral(1)),
        }]);
        // `Return(Some(value))`.
        assert_monomorphize_propagates_error(vec![HirStmt::Return(Some(bad_generic_call()))]);
        // `AttrSet`'s own `base`/`value` (D-154) -- both share the same
        // `for sub in [base, value]` loop, so a single erroring `base` case
        // exercises the loop's own `?`.
        assert_monomorphize_propagates_error(vec![HirStmt::AttrSet {
            base: bad_generic_call(),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(0),
        }]);
        assert_monomorphize_propagates_error(vec![HirStmt::AttrSet {
            base: HirExpr::IntLiteral(0),
            attr: "x".to_string(),
            value: bad_generic_call(),
        }]);
    }

    #[test]
    fn monomorphize_defends_a_for_list_over_a_non_container_binding_defensively() {
        // Bypasses ordinary validation deliberately -- `for x in xs` where
        // `xs: int` would already be rejected by `check`/`check_and_resolve`
        // before `monomorphize` ever ran. Calling `monomorphize` directly
        // exercises its own defensive fallback for an already-invalid
        // binding, matching this file's existing "defense in depth" test
        // convention (see e.g. `instantiate_generic_call_rejects_two_distinct_type_parameters`).
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![("xs".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "x".to_string(),
                list: "xs".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })],
            }],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = monomorphize(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    #[test]
    fn monomorphize_defends_a_comprehension_over_a_non_container_binding_defensively() {
        // Same defensive-fallback rationale as the `ForList` test above,
        // for `rewrite_comp_iter`'s own `CompIter::Name` fallback arm.
        let param = Ty::Param(Box::new("T".to_string()));
        let identity = generic_identity_fn(param.clone(), param);
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![("xs".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "ys".to_string(),
                var: "0comp_v".to_string(),
                iter: CompIter::Name("xs".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                }),
            }],
        };
        let hir = HirModule {
            items: vec![identity, f],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let resolved = monomorphize(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_identity__T_int"), 1);
    }

    // ---------------------------------------------------------------
    // PR-13 final review fixes: self/mutual generic recursion (Critical),
    // consistent `type_aliases` on both `check_and_resolve` paths (I1),
    // and a generic body seeing its non-generic siblings (I3).
    // ---------------------------------------------------------------

    /// `def rec[T](x: T, n: int) -> T` whose body calls `rec` again --
    /// the exact shape that used to be accepted by `check` and then ICE
    /// in `pycc_mir` during `build`.
    fn self_recursive_generic_module() -> HirModule {
        let param = Ty::Param(Box::new("T".to_string()));
        HirModule {
            items: vec![
                HirItem::Function {
                    name: "rec".to_string(),
                    params: vec![("x".to_string(), param.clone()), ("n".to_string(), Ty::Int)],
                    return_ty: param,
                    body: vec![
                        HirStmt::If {
                            test: HirExpr::Compare {
                                op: CmpOpKind::Gt,
                                left: Box::new(HirExpr::Name("n".to_string())),
                                right: Box::new(HirExpr::IntLiteral(0)),
                            },
                            body: vec![HirStmt::Return(Some(HirExpr::Call {
                                callee: "rec".to_string(),
                                args: vec![
                                    HirExpr::Name("x".to_string()),
                                    HirExpr::BinOp {
                                        op: BinOpKind::Sub,
                                        left: Box::new(HirExpr::Name("n".to_string())),
                                        right: Box::new(HirExpr::IntLiteral(1)),
                                    },
                                ],
                            }))],
                            orelse: vec![],
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "rec".to_string(),
                        args: vec![HirExpr::IntLiteral(5), HirExpr::IntLiteral(3)],
                    }],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        }
    }

    /// A call to the enclosing generic function itself -- the shape
    /// `reject_generic_calls_in_block` must find wherever it is nested.
    fn self_call() -> HirExpr {
        HirExpr::Call {
            callee: "f".to_string(),
            args: vec![],
        }
    }

    fn benign() -> HirExpr {
        HirExpr::IntLiteral(0)
    }

    fn assert_self_call_found(stmt: HirStmt) {
        let err = reject_generic_calls_in_block(&Environment::new(), "f", &[stmt]).unwrap_err();
        assert_eq!(err.code, "T0042");
        assert!(err.message.contains("calls itself"));
    }

    #[test]
    fn the_generic_recursion_gate_finds_a_self_call_in_every_statement_position() {
        // `reject_generic_calls_in_stmt` mirrors
        // `rewrite_generic_calls_in_stmt`'s own exhaustive statement walk;
        // every arm must actually reach the calls it holds, so each one
        // gets a case here (D-014's region gate would otherwise be
        // satisfied by an arm that silently visits nothing).
        let cases = vec![
            HirStmt::ExprStmt(self_call()),
            HirStmt::Assign {
                target: "t".to_string(),
                value: self_call(),
            },
            HirStmt::AnnAssign {
                target: "t".to_string(),
                annotation: Ty::Int,
                value: Some(self_call()),
            },
            HirStmt::Return(Some(self_call())),
            HirStmt::If {
                test: self_call(),
                body: vec![],
                orelse: vec![],
            },
            HirStmt::If {
                test: benign(),
                body: vec![HirStmt::ExprStmt(self_call())],
                orelse: vec![],
            },
            HirStmt::If {
                test: benign(),
                body: vec![],
                orelse: vec![HirStmt::ExprStmt(self_call())],
            },
            HirStmt::While {
                test: self_call(),
                body: vec![],
            },
            HirStmt::While {
                test: benign(),
                body: vec![HirStmt::ExprStmt(self_call())],
            },
            HirStmt::ForRange {
                var: "i".to_string(),
                start: self_call(),
                stop: benign(),
                step: benign(),
                body: vec![],
            },
            HirStmt::ForRange {
                var: "i".to_string(),
                start: benign(),
                stop: benign(),
                step: benign(),
                body: vec![HirStmt::ExprStmt(self_call())],
            },
            HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![HirStmt::ExprStmt(self_call())],
            },
            HirStmt::DictSet {
                dict: "d".to_string(),
                key: self_call(),
                value: benign(),
            },
            HirStmt::DictSet {
                dict: "d".to_string(),
                key: benign(),
                value: self_call(),
            },
            HirStmt::ListCompAssign {
                target: "ys".to_string(),
                var: "v".to_string(),
                iter: CompIter::Range {
                    start: self_call(),
                    stop: benign(),
                    step: benign(),
                },
                cond: None,
                elt: Box::new(benign()),
            },
            HirStmt::ListCompAssign {
                target: "ys".to_string(),
                var: "v".to_string(),
                iter: CompIter::Name("xs".to_string()),
                cond: Some(Box::new(self_call())),
                elt: Box::new(benign()),
            },
            HirStmt::SetCompAssign {
                target: "ys".to_string(),
                var: "v".to_string(),
                iter: CompIter::Name("xs".to_string()),
                cond: None,
                elt: Box::new(self_call()),
            },
            HirStmt::DictCompAssign {
                target: "ys".to_string(),
                var: "v".to_string(),
                iter: CompIter::Range {
                    start: benign(),
                    stop: benign(),
                    step: benign(),
                },
                cond: Some(Box::new(benign())),
                key: Box::new(self_call()),
                value: Box::new(benign()),
            },
            HirStmt::DictCompAssign {
                target: "ys".to_string(),
                var: "v".to_string(),
                iter: CompIter::Name("xs".to_string()),
                cond: None,
                key: Box::new(benign()),
                value: Box::new(self_call()),
            },
        ];
        for case in cases {
            assert_self_call_found(case);
        }
    }

    #[test]
    fn the_generic_recursion_gate_finds_a_self_call_in_every_expression_position() {
        // Same exhaustiveness requirement one level down, for
        // `reject_generic_calls_in_expr`. Every recursive position gets a
        // case whose call is exactly there, so no arm can regress into
        // silently skipping a sub-expression.
        let cases = vec![
            HirExpr::Call {
                callee: "other".to_string(),
                args: vec![self_call()],
            },
            HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(self_call()),
                right: Box::new(benign()),
            },
            HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(benign()),
                right: Box::new(self_call()),
            },
            HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(benign()),
                right: Box::new(self_call()),
            },
            HirExpr::FString(vec![
                FStringPart::Literal("x".to_string()),
                FStringPart::Interpolation(Box::new(self_call())),
            ]),
            HirExpr::ListLiteral(vec![self_call()]),
            HirExpr::SetLiteral(vec![self_call()]),
            HirExpr::TupleLiteral(vec![self_call()]),
            HirExpr::DictLiteral(vec![(self_call(), benign())]),
            HirExpr::DictLiteral(vec![(benign(), self_call())]),
            HirExpr::Subscript {
                base: Box::new(self_call()),
                index: Box::new(benign()),
            },
            HirExpr::Subscript {
                base: Box::new(benign()),
                index: Box::new(self_call()),
            },
            HirExpr::Slice {
                base: Box::new(self_call()),
                start: None,
                stop: None,
                step: None,
            },
            HirExpr::Slice {
                base: Box::new(benign()),
                start: Some(Box::new(benign())),
                stop: Some(Box::new(benign())),
                step: Some(Box::new(self_call())),
            },
            HirExpr::ListAppend {
                list: "xs".to_string(),
                value: Box::new(self_call()),
            },
            HirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(self_call()),
            },
            HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(self_call()),
                default: Box::new(benign()),
            },
            HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(benign()),
                default: Box::new(self_call()),
            },
        ];
        for case in cases {
            assert_self_call_found(HirStmt::ExprStmt(case));
        }
    }

    #[test]
    fn the_generic_recursion_gate_accepts_a_body_with_no_generic_call_anywhere() {
        // The success tail of every arm above: the same shapes, none of
        // which contains a self-call or a call to a registered generic.
        let leaves = vec![
            HirExpr::IntLiteral(1),
            HirExpr::FloatLiteral(1.0),
            HirExpr::BoolLiteral(true),
            HirExpr::StringLiteral("s".to_string()),
            HirExpr::Name("x".to_string()),
            HirExpr::ListPop {
                list: "xs".to_string(),
            },
            HirExpr::Call {
                callee: "other".to_string(),
                args: vec![benign()],
            },
            HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(benign()),
                right: Box::new(benign()),
            },
            HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(benign()),
                right: Box::new(benign()),
            },
            HirExpr::FString(vec![
                FStringPart::Literal("x".to_string()),
                FStringPart::Interpolation(Box::new(benign())),
            ]),
            HirExpr::ListLiteral(vec![benign()]),
            HirExpr::SetLiteral(vec![benign()]),
            HirExpr::TupleLiteral(vec![benign()]),
            HirExpr::DictLiteral(vec![(benign(), benign())]),
            HirExpr::Subscript {
                base: Box::new(benign()),
                index: Box::new(benign()),
            },
            HirExpr::Slice {
                base: Box::new(benign()),
                start: Some(Box::new(benign())),
                stop: None,
                step: None,
            },
            HirExpr::ListAppend {
                list: "xs".to_string(),
                value: Box::new(benign()),
            },
            HirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(benign()),
            },
            HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(benign()),
                default: Box::new(benign()),
            },
        ];
        let mut body: Vec<HirStmt> = leaves.into_iter().map(HirStmt::ExprStmt).collect();
        body.push(HirStmt::AnnAssign {
            target: "t".to_string(),
            annotation: Ty::Int,
            value: None,
        });
        body.push(HirStmt::Return(None));
        assert!(reject_generic_calls_in_block(&Environment::new(), "f", &body).is_ok());
    }

    #[test]
    fn check_rejects_a_self_recursive_generic_function() {
        // The validation-only entry point must reject this, not just
        // `check_and_resolve`: the whole defect was `check` (i.e. `pycc
        // check`) accepting a program `pycc build` then panicked on.
        let err = check(&self_recursive_generic_module()).unwrap_err();
        assert_eq!(err.code, "T0042");
        assert!(
            err.message.contains("calls itself"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn check_and_resolve_rejects_a_self_recursive_generic_function() {
        // The `build` half of the same gap: `check_and_resolve` is what
        // produces the HIR `pycc_mir` consumes, so the old panic path
        // ("`$fn:rec` has no recorded type") is provably unreachable only
        // if this errors before `monomorphize` runs.
        let err = check_and_resolve(&self_recursive_generic_module()).unwrap_err();
        assert_eq!(err.code, "T0042");
        assert!(err.message.contains("calls itself"));
    }

    #[test]
    fn check_rejects_a_generic_function_calling_another_generic_function() {
        // Indirect/mutual recursion between two generic functions is
        // rejected by the same gate: `f` cannot be proven non-recursive
        // without a whole-module call-graph analysis D-134's thin slice
        // does not have.
        let param = Ty::Param(Box::new("T".to_string()));
        let g = HirItem::Function {
            name: "g".to_string(),
            params: vec![("y".to_string(), param.clone())],
            return_ty: param.clone(),
            body: vec![HirStmt::Return(Some(HirExpr::Name("y".to_string())))],
        };
        let f = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), param.clone())],
            return_ty: param,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "g".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            }))],
        };
        // `g` is declared *after* `f`, proving the gate does not depend on
        // source order (pass 1 binds every signature before any body runs).
        let hir = HirModule {
            items: vec![f, g],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0042");
        assert!(
            err.message.contains("calls generic function `g`"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn a_generic_function_body_can_call_a_non_generic_sibling() {
        // I3: this used to report a factually false `T0021` "call to
        // undefined function `helper`" because the generic body was
        // checked without the module's function environment.
        let param = Ty::Param(Box::new("T".to_string()));
        let helper = HirItem::Function {
            name: "helper".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("n".to_string())),
                right: Box::new(HirExpr::IntLiteral(1)),
            }))],
        };
        let g = HirItem::Function {
            name: "g".to_string(),
            params: vec![("x".to_string(), param.clone())],
            return_ty: param,
            body: vec![
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    }],
                }),
                HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
            ],
        };
        let hir = HirModule {
            items: vec![
                helper,
                g,
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "g".to_string(),
                        args: vec![HirExpr::IntLiteral(7)],
                    }],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(count_function(&resolved, "0gen_g__T_int"), 1);
    }

    #[test]
    fn check_and_resolve_returns_empty_type_aliases_on_both_paths() {
        // I1: a resolved module's `type_aliases` value must not depend on
        // whether the module happened to contain a generic function.
        // D-135 aliases are fully discharged during HIR lowering, so the
        // resolved HIR's own field is empty by design on both paths.
        let param = Ty::Param(Box::new("T".to_string()));
        let aliases = vec![("MyInt".to_string(), Ty::Int)];
        let non_generic = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            }],
            type_aliases: aliases.clone(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let generic = HirModule {
            items: vec![
                generic_identity_fn(param.clone(), param),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: aliases, imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(
            check_and_resolve(&non_generic)
                .unwrap()
                .type_aliases
                .is_empty(),
            "no-generics path must discharge `type_aliases`"
        );
        assert!(
            check_and_resolve(&generic).unwrap().type_aliases.is_empty(),
            "monomorphization path must discharge `type_aliases`"
        );
    }

    // ---- Issue #22 review fixes: incompatible redefinition rejection ----

    #[test]
    fn incompatible_redefinition_with_different_param_count_is_rejected() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![
                        ("x".to_string(), Ty::Int),
                        ("y".to_string(), Ty::Int),
                    ],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn checked_function_signatures_rejects_incompatible_redefinition() {
        // Exercises the fast path in checked_function_signatures that calls
        // check_incompatible_redefinitions before trying concrete or solver.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![
                        ("x".to_string(), Ty::Int),
                        ("y".to_string(), Ty::Int),
                    ],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = checked_function_signatures(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn incompatible_redefinition_with_different_return_type_is_rejected() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn incompatible_redefinition_with_different_param_type_is_rejected() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn compatible_redefinition_with_same_signature_is_accepted() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(2)],
                    })],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn check_incompatible_redefinitions_rejects_infer_signature_mismatch() {
        // Issue #402: a same-arity redefinition where one signature still
        // carries Ty::Infer (unresolved by the solver) and the other is
        // concrete must be rejected, not silently accepted. Ty::Infer is an
        // ordinary unit variant under Ty's derived PartialEq, so it only
        // ever equals another Ty::Infer at the same position -- comparing
        // it unconditionally against check_incompatible_redefinitions's
        // `seen` map correctly flags this as a mismatch.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn check_and_resolve_rejects_the_issue_402_reproduction_fixture() {
        // Issue #402's own reproduction fixture, exercised through
        // check_and_resolve (the `pycc build` path): first `foo` is fully
        // unannotated (Ty::Infer for both the parameter and the return
        // type), second `foo` is fully concrete and a different shape.
        // This is now rejected by the pre-resolution
        // check_incompatible_redefinitions call inside
        // checked_function_signatures, before check_and_resolve ever
        // builds a resolved HIR.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo`"),
            "got: {}",
            err.message
        );
        assert!(
            err.message.contains("<inferred>"),
            "message should render the unresolved first signature's Ty::Infer \
             positions as `<inferred>`, got: {}",
            err.message
        );
    }

    #[test]
    fn checked_function_signatures_rejects_the_issue_402_reproduction_fixture() {
        // Same fixture as check_and_resolve_rejects_the_issue_402_reproduction_fixture
        // and check_incompatible_redefinitions_rejects_infer_signature_mismatch,
        // but through `checked_function_signatures`'s own separate call site
        // (mirrors checked_function_signatures_rejects_incompatible_redefinition
        // above, which exercises that same call site with a fully-concrete,
        // different-arity fixture instead of an Infer-involving one).
        // `check_incompatible_redefinitions` is called directly from both
        // `check` and `checked_function_signatures`; each entry point is
        // exercised independently rather than relying on one to cover the
        // other.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let local_names = module_function_local_names(&hir);
        let err = checked_function_signatures(&hir, &local_names).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo`"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn infer_signature_redefinition_is_rejected_regardless_of_body_evidence() {
        // Issue #402: the rejection is structural, based on each
        // definition's raw pre-resolution shape -- not on what the solver
        // would eventually infer from the body. The first `foo`'s body
        // would resolve `x`/the return type to `int` if it ran, but the
        // second `foo` is concretely `(str) -> str`; the raw shapes
        // (Infer, Infer) vs (Str, Str) already disagree, so this is
        // rejected before the solver ever sees the first body.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Str)],
                    return_ty: Ty::Str,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo`"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn compatible_infer_signature_redefinition_with_call_site_evidence_is_accepted() {
        // Issue #402 must-not-regress: two structurally identical
        // Ty::Infer signatures (an unannotated helper redefined verbatim)
        // must still be accepted -- the fix only rejects a *mismatch*
        // between raw shapes, not an unresolved shape on its own. A call
        // site is required so the solver has evidence to resolve `x`/the
        // return type; without one, this fixture would fail for an
        // unrelated reason ("cannot infer type of parameter").
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "foo".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check_and_resolve(&hir).is_ok());
    }

    #[test]
    fn check_and_resolve_rejects_incompatible_redefinition_with_infer_signature() {
        // Issue #402: two functions share the name `foo`; the first has
        // Ty::Infer, the second has a concrete but incompatible (different
        // arity) signature. Before this fix, only a resolved-arity mismatch
        // like this one was caught, and only by a post-resolution recheck
        // inside `check_and_resolve`. That recheck is gone now: this raw
        // shape mismatch (arity, independent of Infer) is caught by the
        // pre-resolution `check_incompatible_redefinitions` call inside
        // `checked_function_signatures`, reached before `check_and_resolve`
        // ever builds a resolved HIR.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![
                        ("x".to_string(), Ty::Int),
                        ("y".to_string(), Ty::Int),
                    ],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn incompatible_redefinition_with_bare_return_type_is_rejected() {
        // Issue #402's most plausible real-world trip (see the plan's own
        // §3.5): a first definition that annotates its parameter but
        // leaves the return type bare (Ty::Infer), redefined with a fully
        // concrete signature that adds a return annotation. Params agree;
        // only the return position differs by Infer-vs-concrete -- this
        // isolates that the fix triggers on the return position alone.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Infer,
                    body: vec![],
                },
                HirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("cannot redefine function `foo` with a different signature"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn mangled_method_name_does_not_collide_with_same_named_top_level_function() {
        // Issue #402 §3.6 regression: `check_incompatible_redefinitions`
        // iterates the same flat `hir.items` list a class's own methods are
        // merged into, name-keyed with no class metadata. A method always
        // lowers into a mangled `<ClassName>.<method_name>` name (never
        // containing a bare `.`-free `NAME` token), so it can never
        // conflate with a bare top-level function of the unmangled name --
        // confirmed here with deliberately *different* signatures: if the
        // name-keyed map ever conflated `"Foo.bar"` and `"bar"`, this
        // fixture would trip T0021 for a differing param type "under the
        // same name". Accepting it is a stronger check than accepting two
        // identical signatures would be.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "Foo.bar".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "bar".to_string(),
                    params: vec![("x".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![],
                },
            ],
            type_aliases: Vec::new(), imports: Vec::new(), class_defs: Vec::new(),
        };
        assert!(check_and_resolve(&hir).is_ok());
    }

    // ---- Issue #359 (Part 2 of #118): solver definite-assignment tracking ----

    #[test]
    fn solver_if_no_else_marks_body_only_binding_as_maybe() {
        // `def _f(cond: bool):` with `if cond: x = 1` (no else) — x is
        // maybe-bound. The solver should track x in `maybe_bindings` so
        // `return x` does not unify the return type with x's type.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("cond".to_string(), Ok(Ty::Bool))]),
            local_names: &["cond", "x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::If {
            test: HirExpr::Name("cond".to_string()),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.maybe_bindings.contains("x"));
        assert!(env.bindings.contains_key("x"));
    }

    #[test]
    fn solver_if_with_else_marks_both_branch_binding_as_definite() {
        // `if cond: x = 1` + `else: x = 2` — both branches bind x, so x
        // is definitely bound (not in `maybe_bindings`).
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("cond".to_string(), Ok(Ty::Bool))]),
            local_names: &["cond", "x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::If {
            test: HirExpr::Name("cond".to_string()),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(2),
            }],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(!env.maybe_bindings.contains("x"));
        assert!(env.bindings.contains_key("x"));
    }

    #[test]
    fn solver_if_no_else_does_not_leak_binding_into_orelse() {
        // Before #359, both branches shared the same env, so a binding
        // from the body leaked into the orelse. With cloning, the orelse
        // branch should NOT see x from the body.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("cond".to_string(), Ok(Ty::Bool))]),
            local_names: &["cond", "x", "y"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        // Body assigns x; orelse assigns y. After the if, both x and y
        // should be maybe-bound (each was introduced by only one branch).
        let body = vec![HirStmt::If {
            test: HirExpr::Name("cond".to_string()),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::IntLiteral(2),
            }],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.maybe_bindings.contains("x"));
        assert!(env.maybe_bindings.contains("y"));
        assert!(env.bindings.contains_key("x"));
        assert!(env.bindings.contains_key("y"));
    }

    #[test]
    fn solver_while_loop_marks_body_only_binding_as_maybe() {
        // A `while` body may execute zero times, so x assigned only in
        // the body is maybe-bound after the loop.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("cond".to_string(), Ok(Ty::Bool))]),
            local_names: &["cond", "x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::While {
            test: HirExpr::Name("cond".to_string()),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.maybe_bindings.contains("x"));
        assert!(env.bindings.contains_key("x"));
    }

    #[test]
    fn solver_for_range_marks_loop_variable_as_maybe() {
        // A `for` loop may execute zero times, so the loop variable is
        // maybe-bound after the loop (if newly introduced).
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["i"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.maybe_bindings.contains("i"));
        assert_eq!(env.bindings.get("i"), Some(&Ok(Ty::Int)));
    }

    #[test]
    fn solver_for_range_body_only_binding_is_maybe() {
        // A binding introduced only in the for-loop body is maybe-bound
        // after the loop (the loop may not execute).
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["i", "x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.maybe_bindings.contains("i"));
        assert!(env.maybe_bindings.contains("x"));
    }

    #[test]
    fn solver_for_range_pre_existing_binding_stays_definite() {
        // A binding that existed before the loop stays definite (not in
        // maybe_bindings) after the loop.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut constraints = SolverConstraints::default();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("x".to_string(), Ok(Ty::Int))]),
            local_names: &["i", "x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };
        let body = vec![HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        // x was pre-existing → stays definite (not maybe)
        assert!(!env.maybe_bindings.contains("x"));
        // i was newly introduced → maybe
        assert!(env.maybe_bindings.contains("i"));
    }

    #[test]
    fn solver_maybe_bound_name_skips_unification_in_name_arm() {
        // A maybe-bound name returns Ok(None) from the Name arm, so
        // `return x` does not unify the return type with x's type.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("x".to_string(), Ok(Ty::Int))]),
            local_names: &["x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::from(["x".to_string()]),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &HirExpr::Name("x".to_string()),
        )
        .unwrap();

        // Ok(None) — the type term is not available for unification
        assert_eq!(term, None);
    }

    #[test]
    fn solver_definitely_bound_name_returns_its_term() {
        // A definitely-bound name (not in maybe_bindings) returns its
        // type term as before.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("x".to_string(), Ok(Ty::Int))]),
            local_names: &["x"],
            defs_rebound: HashSet::new(),
            maybe_bindings: HashSet::new(),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &HirExpr::Name("x".to_string()),
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn solver_private_helper_with_maybe_bound_return_does_not_infer_wrong_type() {
        // End-to-end: a private helper with `if cond: x = 1; return x`
        // should not infer a wrong return type from the maybe-bound x.
        // The solver conservatively refuses to infer the return type
        // (T0021 "cannot infer return type") because `return x` skips
        // unification when x is maybe-bound. With an explicit annotation,
        // the validation pass catches T0041 separately (see the next test).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("cond".to_string(), Ty::Bool)],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::Name("cond".to_string()),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                        orelse: vec![],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        // The solver cannot infer the return type because x is maybe-bound
        // and `return x` skips unification. This is the conservative
        // behavior the issue requires.
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot infer return type"));
    }

    #[test]
    fn solver_private_helper_with_annotation_catches_t0041_for_maybe_bound() {
        // End-to-end: with an explicit return annotation, the solver
        // doesn't need to infer the return type, so the validation pass
        // runs and catches T0041 for the maybe-bound read of x.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("cond".to_string(), Ty::Bool)],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::Name("cond".to_string()),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                        orelse: vec![],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0041");
    }

    #[test]
    fn solver_private_helper_with_both_branches_binding_infers_correctly() {
        // End-to-end: a private helper with both if-branches binding x
        // should infer the return type correctly (x is definitely bound).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![("cond".to_string(), Ty::Bool)],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::Name("cond".to_string()),
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
            }],
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        };
        assert!(check(&hir).is_ok());
    }
}
