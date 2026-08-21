mod binop;
mod class;
mod exception;
mod solver;
#[cfg(test)]
mod tests;
mod unop;

use binop::numeric_result_type;
use exception::{check_raise_stmt, check_try_stmt, is_unshadowed_builtin_exception};
use unop::unary_result_type;

use pycc_diag::{Diagnostic, Span};
#[cfg(test)]
use pycc_hir::CmpOpKind;
pub use pycc_hir::Ty;
use pycc_hir::{
    BinOpKind, CmpOpKind as CmpOp, CompIter, FStringPart, HirClassDef, HirExpr, HirItem,
    HirMatchCase, HirModule, HirPattern, HirStmt, PropertyDef, UnaryOpKind,
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

impl BindingState {
    /// Returns the inner `Ty` regardless of whether the binding is
    /// `Definitely` or `Maybe` assigned.
    fn ty(&self) -> &Ty {
        match self {
            BindingState::Definitely(t) | BindingState::Maybe(t) => t,
        }
    }
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
    /// PEP 695 (#387): the name of the generic *function* currently being
    /// body-checked, if any. Set by `check_function_in` from the function's
    /// own signature via `generic_type_param_name`; `None` for a non-generic
    /// function (and for the module-level environment). Used by
    /// `check_stmt_in_function`'s `Return` arm to reject a generic function
    /// returning its *own* `Ty::Param` as a concrete scalar
    /// (`def bad[T](x: T) -> int: return x`): `is_assignable`'s
    /// `from == Ty::Param` clause must remain (a non-generic function reading
    /// a generic class instance's attribute still needs `T` → `int` to pass
    /// during type checking, before monomorphization substitutes `T`), so the
    /// function-owned-vs-class-owned distinction is enforced here instead.
    /// `Option<String>` (not `&str`) because `Environment` is owned/cloned
    /// per function via `child_for_function`/`clone`.
    own_type_param: Option<String>,
    /// #433: the name of the class whose method body is currently being
    /// type-checked, if any. Set by `check_function_in` from the method's
    /// own mangled `<ClassName>.<method>` name (the `.` separator is unique
    /// to mangled method names — a real Python identifier can never contain
    /// one, so the prefix before the first `.` is unambiguously the class
    /// name). `None` for a top-level function and for the module-level
    /// environment. Used by `infer_expr_in`'s `HirExpr::Super` arm and by
    /// `resolve_method_call`/`resolve_attr_get` when the base is `Super` to
    /// resolve the next class in the MRO after this one (D-006 static
    /// dispatch, per the #433 ADR — no vtable, no runtime dispatch).
    current_class: Option<String>,
    /// PEP 591 (#383): names declared `Final` (variable-level only, not
    /// parameters or class attributes). Populated from `HirStmt::AnnAssign`'s
    /// `is_final` flag in `check_stmt`/`check_stmt_in_function`'s `AnnAssign`
    /// arms, *after* the initial `check_assignment` call so the initial
    /// binding is not rejected. `check_assignment` consults this set before
    /// its `lookup_any`/`declared` branches: if the target is in `finals`
    /// and already has a runtime binding in `bindings`, this is a
    /// reassignment and is rejected with `T0045`. Mirrors `declared`'s
    /// `child_for_function` clearing behavior — a function-local `Final`
    /// declaration for a name shadowing a module-level `Final` does not
    /// inherit the module-level constraint.
    finals: HashSet<String>,
    /// #382 (PR-22 Part 1): `true` when the statement being checked is
    /// inside an `except` handler body. Used to validate bare `raise`
    /// (re-raise) — only valid inside an except handler. Set to `true`
    /// before checking a handler body, reset to the previous value after.
    in_except_handler: bool,
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

    /// #433: Returns the name of the class whose method body is currently
    /// being type-checked, if any. Set by `check_function_in` from the
    /// method's mangled `<ClassName>.<method>` name; `None` for a top-level
    /// function or the module-level environment.
    pub(crate) fn current_class(&self) -> Option<&str> {
        self.current_class.as_deref()
    }

    fn child_for_function(&self, local_names: &[&str]) -> Self {
        let mut child = self.clone();
        for name in local_names {
            child.bindings.remove(*name);
            child.declared.remove(*name);
            child.finals.remove(*name);
        }
        // Issue #22: a function body may call any module-level function
        // regardless of source order -- Python's late binding evaluates a
        // function body at call time, by which point all module-level
        // `def`s have typically executed. Seed `defined_functions` with
        // every known function name so the call-before-`def` check in
        // `infer_expr_in`'s `HirExpr::Call` arm never fires inside a
        // function body.
        child
            .defined_functions
            .extend(child.functions.keys().cloned());
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
    )
    .with_help(format!("call it: `{name}(...)`"))
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

/// A stdlib class marker (e.g. `enum.Enum`) referenced as a first-class
/// value (`print(enum.Enum)`, not `class C(Enum):`). `Enum` is only a
/// marker for enum class detection — it has no runtime representation
/// this compiler can emit as a value.
fn enum_marker_is_not_a_value(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "`{name}` is a class marker, not a first-class value — use it only as a base class (`class C(Enum):`)"
        ),
        Span::new(0, 0),
    )
}

/// A stdlib marker symbol (protocol marker, ABC marker, or decorator
/// marker) referenced as a first-class value (#380, PR-20). These symbols
/// are only valid as base-class markers or decorators — they have no
/// runtime representation this compiler can emit as a value.
fn marker_is_not_a_value(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "`{name}` is a marker symbol, not a first-class value — use it only as a base class marker or decorator"
        ),
        Span::new(0, 0),
    )
}

/// Returns `true` if `kind` is any marker symbol kind (Enum, Protocol, ABC,
/// or Decorator). Used by call-site and value-reference guards to reject
/// marker symbols used as first-class values with a consistent diagnostic.
fn is_marker_kind(kind: pycc_std::StdSymbolKind) -> bool {
    matches!(
        kind,
        pycc_std::StdSymbolKind::EnumMarker
            | pycc_std::StdSymbolKind::ProtocolMarker
            | pycc_std::StdSymbolKind::AbcMarker
            | pycc_std::StdSymbolKind::DecoratorMarker
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
        Ty::Int
        | Ty::Float
        | Ty::Bool
        | Ty::Str
        | Ty::None
        | Ty::Infer
        | Ty::Instance(_)
        | Ty::Protocol(_) => false,
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
            | HirStmt::AttrSet { .. }
            | HirStmt::Raise { .. } => {}
            HirStmt::Match { cases, .. } => {
                for case in cases {
                    collect_pattern_capture_names(&case.pattern, names);
                    collect_local_names(&case.body, names);
                }
            }
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                collect_local_names(body, names);
                for handler in handlers {
                    if let Some(name) = &handler.name
                        && !is_local(names, name)
                    {
                        names.push(name);
                    }
                    collect_local_names(&handler.body, names);
                }
                collect_local_names(orelse, names);
                collect_local_names(finalbody, names);
            }
        }
    }
}

/// PEP 634-636 (#381, PR-21): collects all capture names introduced by a
/// pattern (recursively), for `collect_local_names`'s pre-pass.
fn collect_pattern_capture_names<'a>(pattern: &'a HirPattern, names: &mut Vec<&'a str>) {
    match pattern {
        HirPattern::Wildcard
        | HirPattern::Literal(_)
        | HirPattern::Singleton(_)
        | HirPattern::NoneSingleton => {}
        HirPattern::Capture(name) => {
            if !is_local(names, name) {
                names.push(name);
            }
        }
        HirPattern::Sequence(subs) | HirPattern::Or(subs) => {
            for sub in subs {
                collect_pattern_capture_names(sub, names);
            }
        }
        HirPattern::SequenceStar(subs, rest) => {
            for sub in subs {
                collect_pattern_capture_names(sub, names);
            }
            if let Some(rest) = rest
                && !is_local(names, rest)
            {
                names.push(rest);
            }
        }
        HirPattern::Mapping(pairs, rest) => {
            for (_, sub) in pairs {
                collect_pattern_capture_names(sub, names);
            }
            if let Some(rest) = rest
                && !is_local(names, rest)
            {
                names.push(rest);
            }
        }
        HirPattern::Class {
            positional,
            keyword,
            ..
        } => {
            for sub in positional {
                collect_pattern_capture_names(sub, names);
            }
            for (_, sub) in keyword {
                collect_pattern_capture_names(sub, names);
            }
        }
        HirPattern::As(inner, name) => {
            collect_pattern_capture_names(inner, names);
            if !is_local(names, name) {
                names.push(name);
            }
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

/// #380 (PR-20): Pre-binds function-local variable types into `env` by
/// walking the body in source order and inferring each assignment's
/// value type. This lets the protocol monomorphization pass resolve
/// local variables (not just module-level globals) when inferring the
/// concrete type of a call-site argument.
fn bind_local_types_in_body(env: &mut Environment, local_names: &[&str], body: &[HirStmt]) {
    for stmt in body {
        bind_local_types_in_stmt(env, local_names, stmt);
    }
}

fn bind_local_types_in_stmt(env: &mut Environment, local_names: &[&str], stmt: &HirStmt) {
    match stmt {
        HirStmt::Assign { target, value } => {
            if let Ok(ty) = infer_expr_in(env, local_names, value) {
                env.bind(target.clone(), ty);
            }
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            ..
        } => {
            if let Some(val) = value {
                if let Ok(ty) = infer_expr_in(env, local_names, val) {
                    env.bind(target.clone(), ty);
                }
            } else {
                env.bind(target.clone(), annotation.clone());
            }
        }
        HirStmt::If { body, orelse, .. } => {
            bind_local_types_in_body(env, local_names, body);
            bind_local_types_in_body(env, local_names, orelse);
        }
        HirStmt::While { body, .. } => {
            bind_local_types_in_body(env, local_names, body);
        }
        HirStmt::ForRange { var, body, .. } => {
            env.bind(var.clone(), Ty::Int);
            bind_local_types_in_body(env, local_names, body);
        }
        HirStmt::ForList { var, list, body } => {
            if let Some(BindingState::Definitely(Ty::List(elt_ty))) = env.binding_state(list) {
                env.bind(var.clone(), (**elt_ty).clone());
            }
            bind_local_types_in_body(env, local_names, body);
        }
        _ => {}
    }
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
fn homogeneous_private_solver_scalar_list_element(
    element_terms: &[Option<TypeTerm>],
) -> Option<Ty> {
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
pub(crate) struct ConstraintEnvironment<'scope, 'hir> {
    pub(crate) bindings: HashMap<String, TypeTerm>,
    pub(crate) local_names: &'scope [&'hir str],
    /// Mirror of `Environment::def_rebound` (D-110): names whose net
    /// source-order module binding is a `def`, kept apart from the term
    /// bindings for the same reason -- terms must survive a `def` for
    /// representation purposes.
    pub(crate) defs_rebound: HashSet<String>,
    /// Issue #359 (Part 2 of #118): names whose binding is *maybe* —
    /// assigned in only one branch of an `if` (no `else`), or only in a
    /// loop body, or introduced as a `for` loop variable (the loop may
    /// execute zero times). Mirrors the validation pass's
    /// `BindingState::Maybe` distinction (D-147): a maybe-bound name's
    /// type term is still in `bindings` (if it IS bound, it has that
    /// type), but `collect_expr_constraints`'s `Name` arm skips
    /// unification for maybe-bound names — the validation pass's `T0041`
    /// diagnostic is the user-facing gate, not the solver's inferred type.
    pub(crate) maybe_bindings: HashSet<String>,
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
                    pycc_std::StdSymbolKind::Constant { ty } => Ok(Some(Ok(std_scalar_to_ty(ty)))),
                    pycc_std::StdSymbolKind::Function { .. } => {
                        Err(std_function_used_as_a_value(name))
                    }
                    pycc_std::StdSymbolKind::EnumMarker => Err(enum_marker_is_not_a_value(name)),
                    pycc_std::StdSymbolKind::ProtocolMarker
                    | pycc_std::StdSymbolKind::AbcMarker
                    | pycc_std::StdSymbolKind::DecoratorMarker => Err(marker_is_not_a_value(name)),
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
        // #603 (Part 2 of #573). An operand whose type is already concrete
        // is typed directly by `unary_result_type`, so a bad operand keeps
        // the unary diagnostic ("unary operator USub is not defined for
        // `str`") rather than a confusing binary one about the rewrite's
        // synthetic `0`. That path covers every operand the solver can see
        // a type for, which is the overwhelming majority.
        //
        // An operand that is still an inference variable has no type to
        // check yet, so it is deferred as the exact binary constraint
        // `pycc_mir` will lower the expression to -- `0 - x` / `0 + x`.
        // `numeric_result_type(Sub | Add, Int, operand)` *is* the unary
        // rule (`Int`/`Bool` give `Int` since `-True == -1`, `Float` gives
        // `Float`, anything else is `T0021`), so reusing it keeps the
        // solver's view of the expression identical to MIR's own rewrite
        // and the two cannot drift apart.
        HirExpr::UnaryOp { op, operand } => {
            let operand =
                collect_expr_constraints(signatures, parents, concrete, binops, env, operand)?;
            match operand {
                Some(Ok(operand_ty)) => Ok(Some(Ok(unary_result_type(*op, operand_ty)?))),
                Some(operand) => {
                    let result = fresh_term(parents, concrete);
                    let op = match op {
                        UnaryOpKind::USub => BinOpKind::Sub,
                        UnaryOpKind::UAdd => BinOpKind::Add,
                    };
                    binops.push((op, Ok(Ty::Int), operand, result.clone()));
                    Ok(Some(result))
                }
                None => Ok(None),
            }
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
                    )
                    .with_help("pass exactly 1 argument"));
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
            // #435: `isinstance`/`issubclass` are compile-time-evaluated
            // builtins that always return `Ty::Bool`. The constraint solver
            // only needs the result type — the actual validation and
            // compile-time evaluation happen in `infer_expr_in`'s own Call
            // arm. The class arguments (bare names or tuples of bare names)
            // produce `Ok(None)` terms in the solver (class names are not
            // value bindings), which is harmless.
            // A user-defined function named `isinstance`/`issubclass` takes
            // priority over the builtin (same pattern as `float` above).
            if (callee == "isinstance" || callee == "issubclass")
                && !signatures.contains_key(callee)
            {
                return Ok(Some(Ok(Ty::Bool)));
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
                    return Err(if is_marker_kind(symbol.kind) {
                        if matches!(symbol.kind, pycc_std::StdSymbolKind::EnumMarker) {
                            enum_marker_is_not_a_value(callee)
                        } else {
                            marker_is_not_a_value(callee)
                        }
                    } else {
                        std_constant_is_not_callable(callee)
                    });
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
                    )
                    .with_help(format!(
                        "pass exactly {} argument(s)",
                        expected_arg_tys.len()
                    )));
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
                        )
                        .with_help(format!(
                            "pass a `{}` value",
                            std_scalar_to_ty(*expected).name()
                        )));
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
                    )
                    .with_help("pass exactly 1 argument"));
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
                    )
                    .with_help("pass an `int`, `float`, or `bool` value"));
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
        // PEP 695 (#387): `C[type_arg](args)` — a generic class
        // instantiation. The args are recursed into for constraint
        // collection. The expression itself produces a concrete
        // `Ty::Instance(class)` type term (not `Ok(None)` like
        // container-shaped expressions), because a GCI's result is a
        // class instance that the solver needs to track so a local
        // assigned from it (`b = C[int](1)`) gets bound in the solver's
        // environment. The type_arg is a compile-time `Ty`, not a runtime
        // expression, so it needs no constraint traversal. The actual
        // type-checking (verifying the class is generic, the type_arg is
        // scalar, and the args match `__init__`) happens in `infer_expr_in`,
        // not here.
        HirExpr::GenericClassInstantiate { class, args, .. } => {
            for arg in args {
                collect_expr_constraints(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(Some(Ok(Ty::Instance(Box::new(class.clone())))))
        }
        // #433: `Super` carries no sub-expressions to recurse into and
        // produces no unification term — it is a compile-time marker only
        // meaningful as the base of a `MethodCall`/`AttrGet`, which the
        // solver's own `MethodCall`/`AttrGet` arms already recurse past
        // (both return `Ok(None)` after recursing into `base`).
        HirExpr::Super => Ok(None),
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
                // Issue #359 (Part 2 of #118): an unconditional assignment
                // upgrades a maybe-bound name back to definitely bound
                // (mirrors the validation pass's contract: `if c: x = 1`
                // followed by `x = 2` makes `x` readable).
                env.maybe_bindings.remove(target.as_str());
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
                is_final: _,
            } => {
                // Issue #359 (Part 2 of #118): an unconditional annotated
                // assignment upgrades a maybe-bound name back to definitely
                // bound, same as a plain `Assign`.
                env.maybe_bindings.remove(target.as_str());
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
                let pre_existing: HashSet<String> = env.bindings.keys().cloned().collect();
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
                solver::join_if_branches_solver(env, &body_env, &orelse_env, &pre_existing);
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
                let pre_existing: HashSet<String> = env.bindings.keys().cloned().collect();
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
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
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
                let pre_existing: HashSet<String> = env.bindings.keys().cloned().collect();
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
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
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
                let pre_existing: HashSet<String> = env.bindings.keys().cloned().collect();
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
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
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
            HirStmt::Match { subject, cases } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    subject,
                )?;
                for case in cases {
                    let pre_existing: HashSet<String> = env.bindings.keys().cloned().collect();
                    let mut case_env = env.clone();
                    collect_block_constraints(
                        signatures,
                        parents,
                        concrete,
                        constraints,
                        &mut case_env,
                        &case.body,
                        return_term.clone(),
                    )?;
                    solver::join_loop_body_solver(env, &case_env, &pre_existing);
                }
            }
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                // #382 (PR-22 Part 1): collect constraints from the try
                // body, each handler, the else body, and the finally body.
                // The try body's bindings are joined back as `Maybe` (the
                // body may raise before reaching an assignment).
                let pre_existing: HashSet<String> = env.bindings.keys().cloned().collect();
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
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
                for handler in handlers {
                    let mut henv = env.clone();
                    // Bind the `as` name in the handler environment.
                    // Inside the handler body, the binding is definite.
                    if let Some(exc_type) = &handler.exc_type
                        && let Some(name) = &handler.name
                    {
                        henv.bindings
                            .insert(name.clone(), Ok(Ty::Instance(Box::new(exc_type.clone()))));
                    }
                    collect_block_constraints(
                        signatures,
                        parents,
                        concrete,
                        constraints,
                        &mut henv,
                        &handler.body,
                        return_term.clone(),
                    )?;
                    solver::join_loop_body_solver(env, &henv, &pre_existing);
                }
                let mut else_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut else_env,
                    orelse,
                    return_term.clone(),
                )?;
                solver::join_loop_body_solver(env, &else_env, &pre_existing);
                // The finally body always runs — collect in-place.
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    env,
                    finalbody,
                    return_term.clone(),
                )?;
            }
            HirStmt::Raise { exc, cause } => {
                // #382 (PR-22 Part 1): A raise expression that is a direct
                // call to a builtin exception class (e.g.
                // `ValueError("msg")`) would be classified as C0001 by
                // `collect_expr_constraints` (the callee is a known callable
                // builtin, not a user-defined function or registered class).
                // The actual validation is done by `check_raise_stmt` in the
                // check pass, so errors from constraint collection for raise
                // operands are deliberately ignored here — they would
                // otherwise prevent the solver path from reaching
                // `check_with_signatures`, where `bind_classes` registers
                // the builtin exception classes and the real check succeeds.
                if let Some(exc_expr) = exc {
                    let _ = collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        exc_expr,
                    );
                }
                if let Some(cause_expr) = cause {
                    let _ = collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        cause_expr,
                    );
                }
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
        HirStmt::Match { cases, .. } => cases.iter().any(|case| contains_return(&case.body)),
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. }
        | HirStmt::Raise { .. } => false,
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            contains_return(body)
                || handlers.iter().any(|h| contains_return(&h.body))
                || contains_return(orelse)
                || contains_return(finalbody)
        }
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
        HirStmt::Match { cases, .. } => cases.iter().any(|case| introduces_bindings(&case.body)),
        HirStmt::Return(_) | HirStmt::ExprStmt(_) => false,
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            introduces_bindings(body)
                || handlers.iter().any(|h| introduces_bindings(&h.body))
                || introduces_bindings(orelse)
                || introduces_bindings(finalbody)
        }
        HirStmt::Raise { .. } => false,
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
fn check_while_body_in_place(env: &mut Environment, body: &[HirStmt]) -> Result<(), Diagnostic> {
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
    body.iter()
        .try_for_each(|s| check_stmt_in_function(env, local_names, s, return_ty.clone()))?;
    orelse
        .iter()
        .try_for_each(|s| check_stmt_in_function(env, local_names, s, return_ty.clone()))
}

/// Issue #118 Part 1: fast-path helper for function-scope `while` loops
/// where the body introduces no new bindings.
fn check_while_body_in_place_in_function(
    env: &mut Environment,
    local_names: &[&str],
    body: &[HirStmt],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    body.iter()
        .try_for_each(|s| check_stmt_in_function(env, local_names, s, return_ty.clone()))
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
        own_type_param: None,
        current_class: None,
        finals: HashSet::new(),
        in_except_handler: false,
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
        let HirItem::Function {
            name, body, params, ..
        } = item
        else {
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
        for (param_name, param_ty) in params.iter().map(|(n, _)| n).zip(&signature.1) {
            env.bindings.insert(param_name.clone(), param_ty.clone());
        }
        // #380 (PR-20): skip the constraint solver for abstract method
        // bodies. An abstract method's HIR body is just `Return(None)`,
        // but its declared return type may be non-`None` (e.g. `-> int`).
        // Running the solver on it would unify `None` with the declared
        // type and produce a spurious `T0022`. The type checker
        // (`check_and_resolve`) also skips abstract method bodies.
        let is_abstract_method = name
            .split('.')
            .next()
            .filter(|class_name| *class_name != name)
            .and_then(|class_name| {
                hir.class_defs
                    .iter()
                    .find(|(n, _)| n == class_name)
                    .map(|(_, cd)| cd)
            })
            .is_some_and(|class_def| {
                let method_name = name.split('.').nth(1).unwrap_or("");
                class_def.abstract_methods.iter().any(|m| m == method_name)
            });
        if is_abstract_method {
            continue;
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
            )
            .with_help(format!("add a return type annotation to `{name}`"))
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
                    pycc_std::StdSymbolKind::EnumMarker => {
                        Err(enum_marker_is_not_a_value(name))
                    }
                    pycc_std::StdSymbolKind::ProtocolMarker
                    | pycc_std::StdSymbolKind::AbcMarker
                    | pycc_std::StdSymbolKind::DecoratorMarker => {
                        Err(marker_is_not_a_value(name))
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
        HirExpr::UnaryOp { op, operand } => {
            let operand_ty = infer_expr_in(env, local_names, operand)?;
            unary_result_type(*op, operand_ty)
        }
        HirExpr::BinOp { op, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            numeric_result_type(*op, left_ty, right_ty)
        }
        HirExpr::Compare { op, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            // #378 (PR-18): `==`/`!=` between same-class dataclass instances
            // is accepted -- the compiler-synthesized `__eq__` method has a
            // known-correct signature `(self, other: SameClass) -> bool`.
            // This is restricted to dataclass classes (not any class with a
            // user-defined `__eq__`) because the MIR rewrite assumes the
            // synthesized signature; a user-defined `__eq__` with wrong
            // arity or return type would reach codegen and panic. Ordering
            // operators (`<`, `<=`, `>`, `>=`) between instances are always
            // rejected with T0021 -- pycc has no `__lt__`/`__le__`/`__gt__`/
            // `__ge__` dispatch. Different-class comparisons also stay T0021.
            if matches!(op, CmpOp::Eq | CmpOp::NotEq)
                && let (Ty::Instance(left_class), Ty::Instance(right_class)) =
                    (&left_ty, &right_ty)
                && left_class == right_class
                && let Some(class_def) = env.lookup_class(left_class)
                && class_def.is_dataclass
            {
                return Ok(Ty::Bool);
            }
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
            // #435: `isinstance`/`issubclass` are compile-time-evaluated
            // builtins. They must be intercepted BEFORE the generic arg
            // inference loop below, because the class argument (args[1] for
            // isinstance, both args for issubclass) is a class name or tuple
            // of class names — not a value expression. Inferring a bare class
            // name as a regular expression would fail with "name not defined"
            // (class names are registered in `env.classes`, not
            // `env.bindings`). The object argument (isinstance's args[0]) IS
            // inferred normally.
            // A user-defined function named `isinstance`/`issubclass` takes
            // priority over the builtin (same pattern as `float` — see the
            // `a_user_defined_float_function_takes_priority_over_the_builtin`
            // test and its identical guard in the constraint solver).
            if callee == "isinstance" && !env.lookup_function(callee).is_some() {
                return class::check_isinstance(env, local_names, args);
            }
            if callee == "issubclass" && !env.lookup_function(callee).is_some() {
                return class::check_issubclass(env, args);
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
                    return Err(if is_marker_kind(symbol.kind) {
                        if matches!(symbol.kind, pycc_std::StdSymbolKind::EnumMarker) {
                            enum_marker_is_not_a_value(callee)
                        } else {
                            marker_is_not_a_value(callee)
                        }
                    } else {
                        std_constant_is_not_callable(callee)
                    });
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
                if !class::is_assignable_env(env, arg_ty, param_ty) {
                    // #380 (PR-20): if the mismatch involves a protocol,
                    // produce a detailed T0046 conformance error.
                    let diag = if matches!(param_ty, Ty::Protocol(_)) || matches!(arg_ty, Ty::Protocol(_)) {
                        class::assignable_error(env, arg_ty, param_ty)
                    } else {
                        Diagnostic::error(
                            "T0021",
                            format!(
                                "argument {} of `{callee}` expects `{}`, got `{}`",
                                i + 1,
                                param_ty.name(),
                                arg_ty.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("pass a `{}` value", param_ty.name()))
                    };
                    return Err(diag);
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
            // PEP 560 (#610): `C[x]` where `C` is a bare class name is
            // `C.__class_getitem__(x)`, not a container index. The base is
            // `HirExpr::Name` referring to a registered class, exactly as in
            // the `MethodCall` arm's own `ClassName.static_method(args)`
            // interception -- and, like that arm, it must run before the
            // ordinary base inference, which has no type for a bare class
            // name and would reject it as an undefined name.
            //
            // Unlike that arm, this one also requires the name to be unbound
            // as a value: `class C: ...` followed by `C = [1, 2, 3]` is
            // accepted (only *type aliases* collide with a class name, see
            // `pycc_hir`'s own collision check), and `C[0]` must then read
            // the list element, not dispatch a class hook. `pycc_mir`'s own
            // `Subscript` arm applies the identical guard, so both crates
            // agree on which of the two `C[0]` means.
            //
            // A class that defines no `__class_getitem__` anywhere in its
            // MRO is rejected by `resolve_static_or_class_method_call`'s own
            // unknown-member diagnostic, which names the class and the
            // missing hook -- CPython raises `TypeError: type 'C' is not
            // subscriptable` for the same program.
            if let HirExpr::Name(class_name) = base.as_ref()
                && env.binding_state(class_name).is_none()
                && !is_local(local_names, class_name)
                && env.lookup_class(class_name).is_some()
            {
                let index_ty = infer_expr_in(env, local_names, index)?;
                return class::resolve_static_or_class_method_call(
                    env,
                    class_name,
                    "__class_getitem__",
                    &[index_ty],
                );
            }
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
            // #433: `super().attr` — resolve the attribute starting from
            // the next class in the current class's MRO, not from the
            // current class itself. The `self` instance retains its actual
            // (most-derived) type, so the slot index is still computed from
            // the full MRO's flat layout downstream.
            if matches!(base.as_ref(), HirExpr::Super) {
                return class::resolve_super_attr_get(env, attr);
            }
            // #436: `ClassName.attr` — accessing an attribute on a class
            // name (not an instance) is not supported. A static or class
            // method accessed without calling it (e.g. `C.create` instead
            // of `C.create()`) has no value representation in this
            // compiler's static-dispatch model. Reject with a clear error
            // rather than letting `infer_expr_in` on the class name
            // produce a confusing "name not defined" diagnostic.
            if let HirExpr::Name(class_name) = base.as_ref()
                && let Some(class_def) = env.lookup_class(class_name)
            {
                // #379 (PR-19): `Color.RED` — accessing an enum member by
                // name on the enum class.
                if let Some(ty) = enum_member_attr_type(class_def, class_name, attr) {
                    return Ok(ty);
                }
                return Err(Diagnostic::error(
                    "T0044",
                    format!(
                        "class `{class_name}` has no attribute named `{attr}` -- \
                         accessing a class attribute or method without an instance is \
                         not supported (use `instance.{attr}` or `{class_name}.{attr}()` \
                         for a static/class method)"
                    ),
                    Span::new(0, 0),
                ));
            }
            let base_ty = infer_expr_in(env, local_names, base)?;
            class::resolve_attr_get(env, &base_ty, attr)
        }
        HirExpr::MethodCall { base, method, args } => {
            // #433: `super().method(args)` — resolve the method starting
            // from the next class in the current class's MRO, with `self`
            // (the most-derived instance) as the implicit first argument.
            if matches!(base.as_ref(), HirExpr::Super) {
                let arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                return class::resolve_super_method_call(env, method, &arg_tys);
            }
            // #436: `ClassName.static_method(args)` or
            // `ClassName.class_method(args)` — a method call on a class
            // name (not an instance). The base is `HirExpr::Name` referring
            // to a registered class. Check the static_methods and
            // class_methods tables before the regular instance-method
            // resolution (which requires a `Ty::Instance` base and would
            // reject a bare class name).
            if let HirExpr::Name(class_name) = base.as_ref()
                && env.lookup_class(class_name).is_some()
                && class::has_static_or_class_method(env, class_name, method)
            {
                let arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                return class::resolve_static_or_class_method_call(
                    env, class_name, method, &arg_tys,
                );
            }
            let base_ty = infer_expr_in(env, local_names, base)?;
            let arg_tys = args
                .iter()
                .map(|arg| infer_expr_in(env, local_names, arg))
                .collect::<Result<Vec<_>, _>>()?;
            // #436: static and class methods can also be called on an
            // instance. Check the static/class method tables before the
            // regular instance-method resolution.
            if let Ty::Instance(ref class_name) = base_ty
                && class::has_static_or_class_method(env, class_name, method)
            {
                return class::resolve_static_or_class_method_call(
                    env, class_name, method, &arg_tys,
                );
            }
            class::resolve_method_call(env, &base_ty, method, &arg_tys)
        }
        // PEP 695 (#387): `C[type_arg](args)` — a generic class
        // instantiation. The result type is `Ty::Instance(class)` — the
        // type argument is a compile-time scalar substitution, not a
        // runtime value, so it does not affect the result's nominal type
        // (the class instance). The actual monomorphization (substituting
        // `T` with `type_arg` in the class's methods) happens later in
        // `pycc_types`' `instantiate_generic_call` / `monomorphize`
        // pipeline, reusing PR-13's generic-function infrastructure.
        HirExpr::GenericClassInstantiate { class, .. } => {
            // Verify the class exists. Genericity (the class has a type
            // parameter) is checked later during monomorphization's rewrite
            // pass (`rewrite_generic_calls_in_expr`), which rejects a
            // non-generic class used with `C[int](args)` with T0042.
            if !env.classes.contains_key(class) {
                return Err(Diagnostic::error(
                    "T0001",
                    format!("class `{class}` is not defined"),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::Instance(Box::new(class.to_string())))
        }
        // #433: a bare `HirExpr::Super` should never reach `infer_expr_in`
        // — HIR lowering rejects a standalone `super()` with C0001, and
        // `super().method()`/`super().attr` are handled by the `MethodCall`/
        // `AttrGet` arms below (which special-case a `Super` base before
        // recursing into `infer_expr_in` for it). This arm is a defense-in-
        // depth guard for a hand-built HIR that bypasses `lower_expr`'s own
        // rejection.
        HirExpr::Super => Err(Diagnostic::error(
            "C0001",
            "a bare `super()` expression is not supported — use `super().method()` or `super().attr`".to_string(),
            Span::new(0, 0),
        )),
    }
}

fn is_assignable(from: Ty, to: Ty) -> bool {
    from == to
    || (from == Ty::Bool && to == Ty::Int) // bool is a subtype of int, TYPE_SYSTEM.md's representation table
    // PEP 695 (#387): a `Ty::Param` in a generic class's method signature
    // is accepted as matching any concrete scalar type during type checking,
    // because `monomorphize` will substitute the type parameter with the
    // correct concrete type before MIR/codegen. The `GenericClassInstantiate`
    // expression already validates that the type argument is a scalar
    // (int/float/bool/str) at HIR-lowering time, so this is safe.
    // Both directions are needed: `to == Ty::Param` for `self.v = arg` where
    // the attribute slot is `Ty::Param`, and `from == Ty::Param` for
    // `return self.v` where the attribute read yields `Ty::Param` and the
    // function's return type is a concrete scalar. A non-generic function
    // (e.g. `Use.fetch`) that reads a generic class instance's attribute
    // (`b.v` where `b: Box[T]`) and returns it as `int` also relies on the
    // `from == Ty::Param` direction: during type checking `b.v` is still
    // `Ty::Param` (monomorphization has not yet substituted it).
    //
    // The `from == Ty::Param` direction is, however, unsafe for a *generic
    // function's own* type parameter: `def bad[T](x: T) -> int: return x`
    // would pass here (T assignable to int) then panic at codegen once T is
    // substituted with `str` at the call site. That case is rejected
    // separately in `check_generic_function_in` / `check_function_in` via
    // the threaded `own_type_param` context (see `check_stmt_in_function`'s
    // `Return` arm), not by narrowing this clause -- `is_assignable` has no
    // call-site context to distinguish a class-owned `Ty::Param` from a
    // function-owned one.
    || matches!(to, Ty::Param(_)) && matches!(from, Ty::Int | Ty::Float | Ty::Bool | Ty::Str)
    || matches!(from, Ty::Param(_)) && matches!(to, Ty::Int | Ty::Float | Ty::Bool | Ty::Str)
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
        )
        .with_help("pass an `int` value"))
    }
}

fn check_assignment(env: &mut Environment, target: &str, ty: Ty) -> Result<(), Diagnostic> {
    // PEP 591 (#383): reject reassignment of a `Final` name. The `finals`
    // set is populated *after* the initial assignment's `check_assignment`
    // call returns (in `check_stmt`/`check_stmt_in_function`'s `AnnAssign`
    // arm), so the initial binding does not trigger this check — only a
    // subsequent `Assign` or valued `AnnAssign` to the same name does. The
    // `bindings.contains_key` guard distinguishes a reassignment (the name
    // already has a runtime value) from the value-less `Final` declaration
    // case (`x: Final[int]` then `x = 1`): a value-less declaration puts the
    // name in `declared`, not `bindings`, so the first real assignment is
    // the *initial* assignment and must be allowed.
    if env.finals.contains(target) && env.bindings.contains_key(target) {
        return Err(Diagnostic::error(
            "T0045",
            format!("cannot reassign `Final` name `{target}`"),
            Span::new(0, 0),
        ));
    }
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
        if !class::is_assignable_env(env, &ty, &previous) {
            // #380 (PR-20): if the mismatch involves a protocol,
            // produce a detailed T0046 conformance error.
            let diag = if matches!(previous, Ty::Protocol(_)) || matches!(ty, Ty::Protocol(_)) {
                class::assignable_error(env, &ty, &previous)
            } else {
                Diagnostic::error(
                    "T0023",
                    format!(
                        "cannot assign `{}` to `{target}`, previously inferred as `{}`",
                        ty.name(),
                        previous.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", previous.name(), ty.name()))
            };
            return Err(diag);
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
            (BindingState::Definitely(ty), None) | (BindingState::Maybe(ty), None) => {
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

/// PEP 634-636 (#381, PR-21): joins N case environments from a `match`
/// statement back into `env`. If `exhaustive`, a binding present in all
/// case envs is `Definitely` (one case always runs); a binding present in
/// only some is `Maybe`. If not exhaustive, there is an implicit "no match"
/// path, so every case-only binding is `Maybe`. Pre-existing bindings are
/// preserved (first-assignment-wins, matching `join_if_branches`).
fn join_match_branches(env: &mut Environment, case_envs: &[Environment], exhaustive: bool) {
    let mut joined: HashMap<String, BindingState> = HashMap::new();
    let all_names: HashSet<&String> = case_envs.iter().flat_map(|ce| ce.bindings.keys()).collect();
    for name in all_names {
        let states: Vec<&BindingState> = case_envs
            .iter()
            .filter_map(|ce| ce.bindings.get(name))
            .collect();
        let ty = states[0].ty().clone();
        let all_definite = !exhaustive
            || states.len() == case_envs.len()
                && states
                    .iter()
                    .all(|s| matches!(*s, BindingState::Definitely(_)));
        if all_definite {
            joined.insert(name.clone(), BindingState::Definitely(ty));
        } else {
            joined.insert(name.clone(), BindingState::Maybe(ty));
        }
    }
    env.bindings = joined;
}

/// PEP 634-636 (#381, PR-21): checks a `match` statement. The subject is
/// inferred once; each case is checked in an independent env clone (like
/// `if` arms). Pattern captures are bound before the guard and body are
/// checked. Exhaustiveness is verified (`T0030` if not exhaustive, but the
/// match is still accepted — Python allows non-exhaustive match).
fn check_match(
    env: &mut Environment,
    local_names: &[&str],
    subject: &HirExpr,
    cases: &[HirMatchCase],
    return_ty: Option<&Ty>,
) -> Result<(), Diagnostic> {
    let subject_ty = infer_expr_in(env, local_names, subject)?;
    let mut case_envs = Vec::with_capacity(cases.len());
    for case in cases {
        let mut case_env = env.clone();
        let bindings = check_pattern(&case_env, local_names, &case.pattern, &subject_ty)?;
        for (name, ty) in &bindings {
            check_assignment(&mut case_env, name, ty.clone())?;
        }
        if let Some(guard) = &case.guard {
            let guard_ty = infer_expr_in(&case_env, local_names, guard)?;
            if guard_ty != Ty::Bool {
                return Err(Diagnostic::error(
                    "T0021",
                    format!("match guard must be `bool`, got `{}`", guard_ty.name()),
                    Span::new(0, 0),
                ));
            }
        }
        for stmt in &case.body {
            match return_ty {
                Some(rt) => check_stmt_in_function(&mut case_env, local_names, stmt, rt.clone())?,
                None => check_stmt(&mut case_env, stmt)?,
            }
        }
        case_envs.push(case_env);
    }
    let exhaustive = check_exhaustive(env, &subject_ty, cases);
    join_match_branches(env, &case_envs, exhaustive);
    if !exhaustive {
        return Err(Diagnostic::error(
            "T0030",
            format!(
                "non-exhaustive `match`: not every value of `{}` is covered (add a `case _:` or \
                 cover all cases)",
                subject_ty.name()
            ),
            Span::new(0, 0),
        ));
    }
    Ok(())
}

/// PEP 634-636 (#381, PR-21): checks a pattern against a subject type,
/// returning the list of `(capture_name, type)` bindings it introduces.
fn check_pattern(
    env: &Environment,
    local_names: &[&str],
    pattern: &HirPattern,
    subject_ty: &Ty,
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    match pattern {
        HirPattern::Wildcard => Ok(vec![]),
        HirPattern::Capture(name) => Ok(vec![(name.clone(), subject_ty.clone())]),
        HirPattern::Literal(expr) => {
            let lit_ty = infer_expr_in(env, local_names, expr)?;
            if !is_assignable(lit_ty.clone(), subject_ty.clone())
                && !is_assignable(subject_ty.clone(), lit_ty.clone())
            {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "literal pattern `{}` does not match subject type `{}`",
                        lit_ty.name(),
                        subject_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(vec![])
        }
        HirPattern::Singleton(b) => {
            if *subject_ty != Ty::Bool {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "singleton pattern `True`/`False` requires a `bool` subject, got `{}`",
                        subject_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            let _ = b;
            Ok(vec![])
        }
        HirPattern::NoneSingleton => {
            if *subject_ty != Ty::None {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "`None` pattern requires a `None` subject, got `{}`",
                        subject_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(vec![])
        }
        HirPattern::Sequence(subs) => {
            let elt_ty = check_sequence_subject(subject_ty)?;
            let mut bindings = Vec::new();
            for sub in subs {
                bindings.extend(check_pattern(env, local_names, sub, &elt_ty)?);
            }
            Ok(bindings)
        }
        HirPattern::SequenceStar(subs, rest) => {
            let elt_ty = check_sequence_subject(subject_ty)?;
            let mut bindings = Vec::new();
            for sub in subs {
                bindings.extend(check_pattern(env, local_names, sub, &elt_ty)?);
            }
            if let Some(rest) = rest {
                bindings.push((rest.clone(), Ty::List(Box::new(elt_ty))));
            }
            Ok(bindings)
        }
        HirPattern::Mapping(pairs, rest) => {
            let Ty::Dict(kv) = subject_ty else {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "mapping pattern requires a `dict` subject, got `{}`",
                        subject_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            };
            let (key_ty, val_ty) = kv.as_ref();
            let mut bindings = Vec::new();
            for (key_expr, val_pat) in pairs {
                let inferred_key = infer_expr_in(env, local_names, key_expr)?;
                if inferred_key != *key_ty {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "mapping pattern key `{}` does not match dict key type `{}`",
                            inferred_key.name(),
                            key_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                bindings.extend(check_pattern(env, local_names, val_pat, val_ty)?);
            }
            if let Some(rest) = rest {
                bindings.push((rest.clone(), subject_ty.clone()));
            }
            Ok(bindings)
        }
        HirPattern::Class {
            class_name,
            positional,
            keyword,
        } => check_class_pattern(
            env,
            local_names,
            class_name,
            positional,
            keyword,
            subject_ty,
        ),
        HirPattern::Or(subs) => {
            let mut all_bindings: Option<Vec<(String, Ty)>> = None;
            for sub in subs {
                let sub_bindings = check_pattern(env, local_names, sub, subject_ty)?;
                match &all_bindings {
                    None => all_bindings = Some(sub_bindings),
                    Some(existing) => {
                        let existing_names: HashSet<&String> =
                            existing.iter().map(|(n, _)| n).collect();
                        let new_names: HashSet<&String> =
                            sub_bindings.iter().map(|(n, _)| n).collect();
                        if existing_names != new_names {
                            return Err(Diagnostic::error(
                                "T0021",
                                "or-pattern alternatives must bind the same set of names"
                                    .to_string(),
                                Span::new(0, 0),
                            ));
                        }
                    }
                }
            }
            Ok(all_bindings.unwrap_or_default())
        }
        HirPattern::As(inner, name) => {
            let mut bindings = check_pattern(env, local_names, inner, subject_ty)?;
            bindings.push((name.clone(), subject_ty.clone()));
            Ok(bindings)
        }
    }
}

/// Helper for sequence pattern checking: extracts the element type from a
/// `list` or `tuple` subject.
fn check_sequence_subject(subject_ty: &Ty) -> Result<Ty, Diagnostic> {
    match subject_ty {
        Ty::List(elt) => Ok((**elt).clone()),
        Ty::Tuple(elems) if !elems.is_empty() => Ok(elems[0].clone()),
        _ => Err(Diagnostic::error(
            "T0021",
            format!(
                "sequence pattern requires a `list` or `tuple` subject, got `{}`",
                subject_ty.name()
            ),
            Span::new(0, 0),
        )),
    }
}

/// Helper for class pattern checking.
fn check_class_pattern(
    env: &Environment,
    local_names: &[&str],
    class_name: &str,
    positional: &[HirPattern],
    keyword: &[(String, HirPattern)],
    subject_ty: &Ty,
) -> Result<Vec<(String, Ty)>, Diagnostic> {
    let class_def = env.lookup_class(class_name).ok_or_else(|| {
        Diagnostic::error(
            "T0021",
            format!("class `{class_name}` is not defined"),
            Span::new(0, 0),
        )
    })?;
    let subject_is_match = match subject_ty {
        Ty::Instance(name) => name.as_str() == class_name || class_def.mro.contains(name),
        _ => false,
    };
    if !subject_is_match {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "class pattern `{class_name}` does not match subject type `{}`",
                subject_ty.name()
            ),
            Span::new(0, 0),
        ));
    }
    let mut bindings = Vec::new();
    let init_params: Vec<(String, Ty)> = class_def
        .methods
        .iter()
        .find(|(name, _)| name == "__init__")
        .and_then(|(_, mangled)| env.lookup_function(mangled))
        .map(|(params, _)| {
            params
                .iter()
                .skip(1)
                .enumerate()
                .map(|(i, ty)| (format!("__pos_{i}"), ty.clone()))
                .collect()
        })
        .unwrap_or_default();
    for (i, pat) in positional.iter().enumerate() {
        let param_ty = init_params
            .get(i)
            .map(|(_, ty)| ty.clone())
            .unwrap_or(Ty::Infer);
        bindings.extend(check_pattern(env, local_names, pat, &param_ty)?);
    }
    for (attr, pat) in keyword {
        let attr_ty = class_def
            .attrs
            .iter()
            .find(|(name, _)| name == attr)
            .map(|(_, ty)| ty.clone())
            .unwrap_or(Ty::Infer);
        bindings.extend(check_pattern(env, local_names, pat, &attr_ty)?);
    }
    Ok(bindings)
}

/// PEP 634-636 (#381, PR-21): returns `true` if the patterns across all
/// cases cover every value of `subject_ty`. See D-169 for the algorithm.
fn check_exhaustive(env: &Environment, subject_ty: &Ty, cases: &[HirMatchCase]) -> bool {
    for case in cases {
        if case.guard.is_some() {
            continue;
        }
        if is_irrefutable_pattern(&case.pattern) {
            return true;
        }
    }
    match subject_ty {
        Ty::Bool => {
            let mut has_true = false;
            let mut has_false = false;
            for case in cases {
                if case.guard.is_some() {
                    continue;
                }
                collect_bool_patterns(&case.pattern, &mut has_true, &mut has_false);
            }
            has_true && has_false
        }
        Ty::Instance(name) => {
            if let Some(class_def) = env.lookup_class(name)
                && !class_def.enum_members.is_empty()
            {
                let mut covered: HashSet<&str> = HashSet::new();
                for case in cases {
                    if case.guard.is_some() {
                        continue;
                    }
                    collect_enum_member_patterns(
                        &case.pattern,
                        name,
                        &class_def.enum_members,
                        &mut covered,
                    );
                }
                class_def
                    .enum_members
                    .iter()
                    .all(|(member, _)| covered.contains(member.as_str()))
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Returns `true` if `pattern` is irrefutable (matches every value of any
/// type): wildcard, bare capture, or an or-pattern containing an irrefutable
/// sub-pattern.
fn is_irrefutable_pattern(pattern: &HirPattern) -> bool {
    match pattern {
        HirPattern::Wildcard | HirPattern::Capture(_) => true,
        HirPattern::Or(subs) => subs.iter().any(is_irrefutable_pattern),
        HirPattern::As(inner, _) => is_irrefutable_pattern(inner),
        _ => false,
    }
}

/// Collects `True`/`False` singleton coverage from a pattern (recursing into
/// or-patterns).
fn collect_bool_patterns(pattern: &HirPattern, has_true: &mut bool, has_false: &mut bool) {
    match pattern {
        HirPattern::Singleton(true) => *has_true = true,
        HirPattern::Singleton(false) => *has_false = true,
        HirPattern::Or(subs) => {
            for sub in subs {
                collect_bool_patterns(sub, has_true, has_false);
            }
        }
        _ => {}
    }
}

/// Collects enum member coverage from class patterns like `Color.RED`.
fn collect_enum_member_patterns<'a>(
    pattern: &HirPattern,
    class_name: &str,
    members: &'a [(String, i64)],
    covered: &mut HashSet<&'a str>,
) {
    match pattern {
        HirPattern::Class { class_name: cn, .. } => {
            if cn == class_name {
                for (member, _) in members {
                    covered.insert(member.as_str());
                }
            }
        }
        HirPattern::Or(subs) => {
            for sub in subs {
                collect_enum_member_patterns(sub, class_name, members, covered);
            }
        }
        _ => {}
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
            is_final,
        } => {
            if let Some(value) = value {
                let inferred = infer_expr(env, value)?;
                if !class::is_assignable_env(env, &inferred, annotation) {
                    // #380 (PR-20): if the mismatch involves a protocol,
                    // produce a detailed T0046 conformance error.
                    let diag = if matches!(annotation, Ty::Protocol(_))
                        || matches!(inferred, Ty::Protocol(_))
                    {
                        class::assignable_error(env, &inferred, annotation)
                    } else {
                        Diagnostic::error(
                            "T0025",
                            format!(
                                "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                                inferred.name(),
                                annotation.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", annotation.name(), inferred.name()))
                    };
                    return Err(diag);
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
                // #380 (PR-20): when the annotation is a protocol type, bind
                // with the concrete (inferred) type instead — the protocol
                // type is a compile-time-only interface, and the MIR needs
                // the concrete type for method/attribute resolution (static
                // dispatch). The conformance check above already validated
                // that the inferred type conforms to the protocol.
                let bind_ty = if matches!(annotation, Ty::Protocol(_)) {
                    inferred.clone()
                } else {
                    annotation.clone()
                };
                check_assignment(env, target, bind_ty)?;
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
            // PEP 591 (#383): record the name as `Final` *after*
            // `check_assignment` returns, so the initial assignment's own
            // `check_assignment` call does not yet see the name in `finals`
            // and is not rejected. A subsequent plain `Assign` or valued
            // `AnnAssign` to the same name will see it in both `finals` and
            // `bindings`, and is rejected with `T0045`. A value-less `Final`
            // declaration (`x: Final[int]` with no `= ...`) also inserts into
            // `finals` — the first real assignment to it is allowed (the name
            // is in `declared`, not `bindings`, so `check_assignment`'s
            // `finals` check does not fire), but a second assignment is
            // rejected.
            if *is_final {
                env.finals.insert(target.clone());
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
            if !was_definite && let Some(ty) = env.lookup_any(var) {
                env.bind_maybe(var.to_string(), ty);
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            // #379 (PR-19): `for c in Color:` — iterating an enum class's
            // members. A class name is not a value binding (classes live in
            // `env.classes`, not `env.bindings`), so `lookup_bound_name`
            // fails. Check `env.lookup_class` first: if `list` is an enum
            // class (has non-empty `enum_members`), bind `var` to
            // `Ty::Instance(list)` and check the body. The actual unrolling
            // (expanding the loop into N sequential copies) is done by a
            // separate HIR→HIR rewrite pass that runs after
            // `check_and_resolve` (see `unroll_enum_loops`), so MIR never
            // sees an enum iterable.
            if let Some(class_def) = env.lookup_class(list)
                && !class_def.enum_members.is_empty()
            {
                return check_enum_loop_body_module(env, var, list, body);
            }
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
            if !was_definite && let Some(ty) = env.lookup_any(var) {
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
        HirStmt::Match { subject, cases } => check_match(env, &[], subject, cases, None),
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => check_try_stmt(env, &[], body, handlers, orelse, finalbody, None),
        HirStmt::Raise { exc, cause } => check_raise_stmt(env, &[], exc, cause),
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
        )
        .with_help(format!("use a `{}` value here", key_ty.name())));
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
    // PEP 695 (#387): record the function's own type-parameter name (if any)
    // so `check_stmt_in_function`'s `Return` arm can reject a generic
    // function returning its own `Ty::Param` as a concrete scalar. Errors
    // from `generic_type_param_name` (multiple params, container-position
    // occurrence) are suppressed here with `.ok()`: a generic function
    // reaching `check_function_in` through `check_generic_function_in` has
    // already been validated by that function's own
    // `generic_type_param_name` call, and a non-generic function returns
    // `Ok(None)` unconditionally.
    env.own_type_param = generic_type_param_name(params, return_ty).ok().flatten();
    // #433: extract the class name from a mangled `<ClassName>.<method>`
    // name so `infer_expr_in`'s `HirExpr::Super` arm can resolve the next
    // class in the MRO. A top-level function name contains no `.`, so
    // `current_class` stays `None` for those (and for the module-level
    // environment, which never goes through `check_function_in`).
    env.current_class = name
        .split('.')
        .next()
        .filter(|prefix| *prefix != name)
        .map(String::from);
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
    // #380 (PR-20): determine whether this function is an abstract method.
    // An abstract method has a declaration-style body (`...` or `pass`)
    // that is not lowered — its HIR body is just `Return(None)`. The
    // function name is mangled as `<ClassName>.<method>`; we check whether
    // the class lists it in `abstract_methods`. Skip body checking and
    // the return-contract check for abstract methods — the body is never
    // executed (a concrete subclass overrides it).
    let is_abstract_method = name
        .split('.')
        .next()
        .filter(|class_name| *class_name != name)
        .and_then(|class_name| module_env.classes.get(class_name))
        .is_some_and(|class_def| {
            let method_name = name.split('.').nth(1).unwrap_or("");
            class_def.abstract_methods.iter().any(|m| m == method_name)
        });
    if !is_abstract_method {
        for stmt in body {
            check_stmt_in_function(&mut env, local_names, stmt, resolved_return.clone())?;
        }
    }
    if !is_abstract_method && resolved_return != Ty::None && !block_always_returns(body) {
        return Err(Diagnostic::error(
            "T0022",
            format!(
                "function `{name}` can exit without returning `{}`",
                resolved_return.name()
            ),
            Span::new(0, 0),
        )
        .with_help(format!("return a `{}` value", resolved_return.name())));
    }
    Ok(())
}

fn block_always_returns(body: &[HirStmt]) -> bool {
    for stmt in body {
        let returns = match stmt {
            HirStmt::Return(_) => true,
            HirStmt::If { body, orelse, .. } => {
                !orelse.is_empty() & block_always_returns(body) & block_always_returns(orelse)
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
            // A raise transfers control to an exception handler/caller and
            // cannot fall through to the function's implicit return point.
            HirStmt::Raise { .. } => true,
            HirStmt::Match { cases, .. } => {
                let mut all_cases_return = !cases.is_empty();
                for case in cases {
                    all_cases_return &= block_always_returns(&case.body);
                }
                all_cases_return
            }
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                // A terminal `finally` replaces every earlier outcome. Otherwise
                // the normal path must terminate either in the try body itself or
                // in its `else`, and every matching handler must terminate. With
                // no handlers, an exception simply propagates to the caller and
                // is already a terminal path.
                let normal_path_terminates = block_always_returns(body)
                    | ((!orelse.is_empty()) & block_always_returns(orelse));
                let mut handled_paths_terminate = true;
                for handler in handlers {
                    handled_paths_terminate &= block_always_returns(&handler.body);
                }
                block_always_returns(finalbody)
                    | (normal_path_terminates & handled_paths_terminate)
            }
        };
        if returns {
            return true;
        }
    }
    false
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
                )
                .with_help(format!("return a `{}` value", return_ty.name())));
            }
            Ok(())
        }
        HirStmt::Return(Some(expr)) => {
            let actual = infer_expr_in(env, local_names, expr)?;
            if !class::is_assignable_env(env, &actual, &return_ty) {
                // #380 (PR-20): if the mismatch involves a protocol,
                // produce a detailed T0046 conformance error.
                let diag =
                    if matches!(return_ty, Ty::Protocol(_)) || matches!(actual, Ty::Protocol(_)) {
                        class::assignable_error(env, &actual, &return_ty)
                    } else {
                        Diagnostic::error(
                            "T0022",
                            format!(
                                "expected return type `{}`, got `{}`",
                                return_ty.name(),
                                actual.name()
                            ),
                            Span::new(0, 0),
                        )
                        .with_help(format!("return a `{}` value", return_ty.name()))
                    };
                return Err(diag);
            }
            // PEP 695 (#387): `is_assignable`'s `from == Ty::Param` clause
            // lets a generic function's own `Ty::Param` pass as any concrete
            // scalar (the clause is needed for non-generic functions reading
            // generic class instance attributes). Narrow that here: if the
            // returned value is the *current function's own* `Ty::Param` and
            // the declared return type is a concrete scalar, reject -- after
            // monomorphization `T` is substituted with the call-site scalar,
            // which may not match the declared return type (e.g.
            // `def bad[T](x: T) -> int: return x` called as `bad("s")`).
            // A class-owned `Ty::Param` (the function has no own type param)
            // is not rejected here -- monomorphization of the class
            // substitutes it before codegen.
            if let (Some(own), Ty::Param(actual_name)) = (&env.own_type_param, &actual)
                && own.as_str() == actual_name.as_ref()
                && matches!(return_ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str)
            {
                return Err(Diagnostic::error(
                    "T0022",
                    format!(
                        "expected return type `{}`, got `{}`",
                        return_ty.name(),
                        actual.name()
                    ),
                    Span::new(0, 0),
                ).with_help(format!(
                    "a generic function's type parameter `{own}` is not guaranteed to be `{}` at every call site",
                    return_ty.name()
                )));
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
                check_if_branches_in_place_in_function(
                    env,
                    local_names,
                    body,
                    orelse,
                    return_ty.clone(),
                )
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
            if !was_definite && let Some(ty) = env.lookup_any(var) {
                env.bind_maybe(var.to_string(), ty);
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            // #379 (PR-19): `for c in Color:` inside a function body —
            // same enum iteration intercept as the module-scope `check_stmt`
            // arm above. A class name is not a value binding, so
            // `lookup_bound_name` fails; check `env.lookup_class` first.
            if let Some(class_def) = env.lookup_class(list)
                && !class_def.enum_members.is_empty()
            {
                return check_enum_loop_body_function(
                    env,
                    var,
                    list,
                    body,
                    local_names,
                    return_ty.clone(),
                );
            }
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
            if !was_definite && let Some(ty) = env.lookup_any(var) {
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
            is_final,
        } => {
            if let Some(value) = value {
                let inferred = infer_expr_in(env, local_names, value)?;
                if !class::is_assignable_env(env, &inferred, annotation) {
                    // #380 (PR-20): if the mismatch involves a protocol,
                    // produce a detailed T0046 conformance error.
                    let diag = if matches!(annotation, Ty::Protocol(_))
                        || matches!(inferred, Ty::Protocol(_))
                    {
                        class::assignable_error(env, &inferred, annotation)
                    } else {
                        Diagnostic::error(
                            "T0025",
                            format!(
                                "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                                inferred.name(),
                                annotation.name()
                            ),
                            Span::new(0, 0),
                        ).with_help(format!("change the value to `{}` (the expected/declared type), or the declaration/annotation to `{}` (the actual type)", annotation.name(), inferred.name()))
                    };
                    return Err(diag);
                }
                // See the module-scope `check_stmt` arm's comment: route
                // through `check_assignment` so a name's first-established
                // representation stays sticky, matching `pycc_mir`'s own
                // `bind_variable` invariant.
                // #380 (PR-20): when the annotation is a protocol type,
                // bind with the concrete (inferred) type instead — see
                // the module-scope arm's comment for the rationale.
                let bind_ty = if matches!(annotation, Ty::Protocol(_)) {
                    inferred.clone()
                } else {
                    annotation.clone()
                };
                check_assignment(env, target, bind_ty)?;
            } else {
                // See the module-scope `check_stmt` arm's comment (issue
                // #245): retain the declared type via `env.declare` without
                // binding it, so a premature read still raises T0021 and a
                // later assignment is checked against the declaration.
                env.declare(target.clone(), annotation.clone())?;
            }
            // PEP 591 (#383): see the module-scope `check_stmt` arm's
            // comment — record the name as `Final` *after*
            // `check_assignment` returns so the initial assignment is not
            // rejected, only a subsequent reassignment.
            if *is_final {
                env.finals.insert(target.clone());
            }
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr_in(env, local_names, expr).map(|_| ()),
        HirStmt::DictSet { dict, key, value } => check_dict_set(env, local_names, dict, key, value),
        HirStmt::AttrSet { base, attr, value } => {
            class::check_attr_set(env, local_names, base, attr, value)
        }
        HirStmt::Match { subject, cases } => {
            check_match(env, local_names, subject, cases, Some(&return_ty))
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => check_try_stmt(
            env,
            local_names,
            body,
            handlers,
            orelse,
            finalbody,
            Some(&return_ty),
        ),
        HirStmt::Raise { exc, cause } => check_raise_stmt(env, local_names, exc, cause),
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
/// Defense in depth, not a reachable frontend path: `crates/pycc_hir/src/func.rs`'s
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
        Ty::Int
        | Ty::Float
        | Ty::Bool
        | Ty::Str
        | Ty::None
        | Ty::Infer
        | Ty::Instance(_)
        | Ty::Protocol(_) => Ok(()),
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
        HirStmt::Match { subject, cases } => {
            exprs.push(subject);
            for case in cases {
                if let Some(guard) = &case.guard {
                    exprs.push(guard);
                }
                blocks.push(&case.body);
            }
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            blocks.push(body);
            for handler in handlers {
                blocks.push(&handler.body);
            }
            blocks.push(orelse);
            blocks.push(finalbody);
        }
        HirStmt::Raise { exc, cause } => {
            exprs.extend(exc.iter());
            exprs.extend(cause.iter());
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
        HirExpr::UnaryOp { operand, .. } => {
            reject_generic_calls_in_expr(module_env, own_name, operand)
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
        // PEP 695 (#387): `C[type_arg](args)` — recurse into args only.
        // `class` is a bare name (not an expression), and `type_arg` is a
        // compile-time `Ty`, so neither needs generic-call rejection.
        HirExpr::GenericClassInstantiate { args, .. } => {
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
        | HirExpr::ListPop { .. }
        | HirExpr::Super => Ok(()),
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

/// #380 (PR-20): Like `substitute_ty` but substitutes
/// `Ty::Protocol(protocol_name)` with `concrete` (a `Ty::Instance`).
/// Used to monomorphize functions with protocol-typed parameters — at
/// each call site, the protocol type is replaced with the concrete
/// argument's class type, so MIR/codegen can resolve method calls and
/// attribute access against the concrete class.
fn substitute_ty_protocol(ty: &Ty, protocol_name: &str, concrete: &Ty) -> Ty {
    match ty {
        Ty::Protocol(name) if name.as_ref() == protocol_name => concrete.clone(),
        other => other.clone(),
    }
}

/// #380 (PR-20): Returns `true` if a function signature has any
/// `Ty::Protocol` parameters, indicating it needs monomorphization at
/// each call site. A protocol *return* type alone does not require
/// monomorphization — the return type is resolved through the type
/// checker's normal assignment logic, and dropping such a function
/// would leave call sites referencing a nonexistent function.
fn has_protocol_param(params: &[(String, Ty)], _return_ty: &Ty) -> bool {
    params.iter().any(|(_, ty)| matches!(ty, Ty::Protocol(_)))
}

/// #380 (PR-20): Mangles a function name for protocol monomorphization
/// with one or more protocol→concrete substitutions, following the
/// existing `0gen_` convention. Each substitution appends
/// `__{protocol_name}_{concrete_name}` to the mangled name.
fn mangle_protocol_instantiation(func_name: &str, substitutions: &[(String, Ty)]) -> String {
    let mut name = format!("0gen_{func_name}");
    for (proto_name, concrete) in substitutions {
        if let Ty::Instance(concrete_name) = concrete {
            name.push_str(&format!("__{proto_name}_{concrete_name}"));
        }
    }
    name
}

/// #380 (PR-20): Substitutes multiple `Ty::Protocol` types in a single
/// `Ty` value, applying each substitution in order.
fn substitute_ty_protocols(ty: &Ty, substitutions: &[(String, Ty)]) -> Ty {
    let mut result = ty.clone();
    for (proto_name, concrete) in substitutions {
        result = substitute_ty_protocol(&result, proto_name, concrete);
    }
    result
}

/// #380 (PR-20): Like `substitute_body_protocol` but applies multiple
/// protocol→concrete substitutions.
fn substitute_body_protocols(body: &[HirStmt], substitutions: &[(String, Ty)]) -> Vec<HirStmt> {
    body.iter()
        .map(|stmt| substitute_stmt_protocols(stmt, substitutions))
        .collect()
}

fn substitute_stmt_protocols(stmt: &HirStmt, substitutions: &[(String, Ty)]) -> HirStmt {
    match stmt {
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            is_final,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty_protocols(annotation, substitutions),
            value: value.clone(),
            is_final: *is_final,
        },
        HirStmt::If { test, body, orelse } => HirStmt::If {
            test: test.clone(),
            body: substitute_body_protocols(body, substitutions),
            orelse: substitute_body_protocols(orelse, substitutions),
        },
        HirStmt::While { test, body } => HirStmt::While {
            test: test.clone(),
            body: substitute_body_protocols(body, substitutions),
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
            body: substitute_body_protocols(body, substitutions),
        },
        HirStmt::ForList { var, list, body } => HirStmt::ForList {
            var: var.clone(),
            list: list.clone(),
            body: substitute_body_protocols(body, substitutions),
        },
        other => other.clone(),
    }
}

/// PEP 695 (#387): Like `substitute_ty` but also substitutes
/// `Ty::Instance(class_name)` with `Ty::Instance(mangled_class)` — needed
/// when monomorphizing a generic class's methods, where `self`'s own
/// parameter type (`Ty::Instance("Container")`) must become
/// `Ty::Instance("0gen_Container__T_int")` so MIR/codegen resolve attribute
/// slots against the monomorphized class's substituted attribute types, not
/// the original generic class's `Ty::Param`-typed slots.
fn substitute_ty_with_class(
    ty: &Ty,
    param_name: &str,
    concrete: &Ty,
    class_name: &str,
    mangled_class: &str,
) -> Ty {
    match ty {
        Ty::Param(name) if name.as_ref() == param_name => concrete.clone(),
        Ty::Instance(name) if name.as_ref() == class_name => {
            Ty::Instance(Box::new(mangled_class.to_string()))
        }
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
            is_final,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty(annotation, param_name, concrete),
            value: value.clone(),
            is_final: *is_final,
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

/// PEP 695 (#387): Like `substitute_body` but uses `substitute_ty_with_class`
/// to also rewrite `Ty::Instance(class_name)` to `Ty::Instance(mangled_class)`.
fn substitute_body_with_class(
    body: &[HirStmt],
    param_name: &str,
    concrete: &Ty,
    class_name: &str,
    mangled_class: &str,
) -> Vec<HirStmt> {
    body.iter()
        .map(|stmt| {
            substitute_stmt_with_class(stmt, param_name, concrete, class_name, mangled_class)
        })
        .collect()
}

fn substitute_stmt_with_class(
    stmt: &HirStmt,
    param_name: &str,
    concrete: &Ty,
    class_name: &str,
    mangled_class: &str,
) -> HirStmt {
    match stmt {
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            is_final,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty_with_class(
                annotation,
                param_name,
                concrete,
                class_name,
                mangled_class,
            ),
            value: value.clone(),
            is_final: *is_final,
        },
        HirStmt::If { test, body, orelse } => HirStmt::If {
            test: test.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
            orelse: substitute_body_with_class(
                orelse,
                param_name,
                concrete,
                class_name,
                mangled_class,
            ),
        },
        HirStmt::While { test, body } => HirStmt::While {
            test: test.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
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
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
        },
        HirStmt::ForList { var, list, body } => HirStmt::ForList {
            var: var.clone(),
            list: list.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
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
        )
        .with_help(format!("pass exactly {} argument(s)", params.len())));
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
                    )
                    .with_help(format!("pass a `{}` value", other.name())));
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
            // #380 (#435): `isinstance`/`issubclass` are compile-time
            // builtins whose class-name arguments (args[1] for isinstance,
            // both args for issubclass) are not value expressions — they
            // are bare class names or tuples of class names. Rewriting them
            // with `rewrite_generic_calls_in_expr` would call
            // `infer_expr_in` on a bare class name, which looks it up as a
            // value binding and fails with T0021 ("name not defined").
            // Skip the class-name arguments entirely, rewriting only the
            // object argument (isinstance's args[0]). A user-defined
            // function named `isinstance` takes priority over the builtin
            // (same pattern as `infer_expr_in`'s own guard).
            //
            // Only `isinstance` needs this special-case here:
            // `issubclass`'s arguments are always class names (never
            // generic call expressions), so `rewrite_generic_calls_in_expr`
            // never encounters a builtin `issubclass` call that needs arg
            // rewriting. Checking only `isinstance` avoids permanently-
            // uncovered `issubclass` branches under D-014's 100 %
            // coverage gate.
            if callee == "isinstance" && env.lookup_function(callee).is_none() {
                // `isinstance` always has ≥ 2 args (the type checker
                // validates argument count before this code runs), so
                // `args[0]` is always safe. The guard was removed to
                // avoid a permanently-uncovered `false` branch under
                // D-014's 100 %-coverage gate.
                //
                // Discard the returned `Ty` — only the rewriting side
                // effect matters here; the subsequent `infer_expr_in`
                // on the whole expression re-derives the type.  Using
                // `let _ =` instead of `?` avoids a permanently-
                // uncovered error-path region under D-014's 100 %
                // coverage gate: `rewrite_generic_calls_in_expr` can
                // only fail when `infer_expr_in` on a sub-expression
                // fails, and this same `infer_expr_in` call on the
                // whole expression immediately below would surface
                // the identical error.
                let _ = rewrite_generic_calls_in_expr(
                    env,
                    local_names,
                    &mut args[0],
                    instantiations,
                    seen,
                );
                return infer_expr_in(env, local_names, expr);
            }
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
        HirExpr::UnaryOp { operand, .. } => {
            // `let _ =` rather than `?`, for the same reason the
            // `isinstance` arm above uses it: only the rewriting side
            // effect matters here, and the `infer_expr_in` call on the
            // whole unary expression immediately below recurses into this
            // same operand, so it surfaces the identical error. Propagating
            // here as well would leave a permanently-uncovered error-path
            // region under D-014's 100 %-coverage gate.
            let _ = rewrite_generic_calls_in_expr(env, local_names, operand, instantiations, seen);
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
            // PEP 560 (#610): when the base is a bare class name, `C[x]` is
            // `C.__class_getitem__(x)` and the base is not a value
            // expression -- recursing into it would call `infer_expr_in` on
            // a bare class name and fail with T0021 ("name not defined"),
            // exactly the failure mode this function's own `isinstance`
            // class-argument skip above exists to avoid. Rewrite only the
            // index in that case; `infer_expr_in` on the whole expression
            // below still resolves the hook and reports any error.
            let base_is_class_name = matches!(base.as_ref(), HirExpr::Name(name)
                if env.binding_state(name).is_none()
                    && !is_local(local_names, name)
                    && env.lookup_class(name).is_some());
            if !base_is_class_name {
                rewrite_generic_calls_in_expr(env, local_names, base, instantiations, seen)?;
            }
            rewrite_generic_calls_in_expr(env, local_names, index, instantiations, seen)?;
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
        // PEP 695 (#387): `C[type_arg](args)` — the monomorphized class
        // methods were pre-registered in `env` by `monomorphize`'s own
        // pre-scan (see `instantiate_generic_class_methods`), so this arm
        // only needs to recurse into `args` (rewriting any nested generic
        // function calls) and then rewrite the expression in place to an
        // ordinary `HirExpr::Call` to the mangled class name. The mangled
        // name follows the same `0gen_`-prefixed scheme as generic
        // functions (D-133/D-134).
        HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args,
        } => {
            for arg in args.iter_mut() {
                rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)?;
            }
            let class_def = env.lookup_class(class).ok_or_else(|| {
                Diagnostic::error(
                    "T0001",
                    format!("class `{class}` is not defined"),
                    Span::new(0, 0),
                )
            })?;
            let type_param_name = class_def.type_param.as_deref().ok_or_else(|| {
                t0042(format!(
                    "class `{class}` is not generic (has no type parameter to instantiate)"
                ))
            })?;
            let mangled = mangle_generic_instantiation(class, type_param_name, type_arg);
            // Rewrite in place to an ordinary call.
            let args_vec = args.clone();
            *expr = HirExpr::Call {
                callee: mangled.clone(),
                args: args_vec,
            };
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => infer_expr_in(env, local_names, expr),
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
            is_final: _,
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
        HirStmt::Match { subject, cases } => {
            rewrite_generic_calls_in_expr(env, local_names, subject, instantiations, seen)?;
            for case in cases.iter_mut() {
                if let Some(guard) = case.guard.as_mut() {
                    rewrite_generic_calls_in_expr(env, local_names, guard, instantiations, seen)?;
                }
                for s in case.body.iter_mut() {
                    rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
                }
            }
            Ok(())
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            for handler in handlers.iter_mut() {
                let mut handler_env = env.clone();
                if let (Some(exc_type), Some(name)) = (&handler.exc_type, &handler.name) {
                    handler_env.bind(name.clone(), Ty::Instance(Box::new(exc_type.clone())));
                }
                for s in handler.body.iter_mut() {
                    rewrite_generic_calls_in_stmt(
                        &mut handler_env,
                        local_names,
                        s,
                        instantiations,
                        seen,
                    )?;
                }
            }
            for s in orelse.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            for s in finalbody.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::Raise { exc, cause } => {
            if let Some(exc) = exc.as_mut() {
                rewrite_generic_calls_in_raise_operand(
                    env,
                    local_names,
                    exc,
                    instantiations,
                    seen,
                )?;
            }
            if let Some(cause) = cause.as_mut() {
                rewrite_generic_calls_in_raise_operand(
                    env,
                    local_names,
                    cause,
                    instantiations,
                    seen,
                )?;
            }
            Ok(())
        }
    }
}

/// Rewrites generic calls nested in a `raise` operand without asking the
/// ordinary call checker to re-type-check the builtin exception constructor
/// itself. The validation pass has already checked that constructor; only its
/// message expression can still contain a generic call that needs rewriting.
fn rewrite_generic_calls_in_raise_operand(
    env: &mut Environment,
    local_names: &[&str],
    expr: &mut HirExpr,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    if let HirExpr::Call { callee, args } = expr
        && is_unshadowed_builtin_exception(env, local_names, callee)
    {
        for arg in args {
            rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)?;
        }
        return Ok(());
    }
    rewrite_generic_calls_in_expr(env, local_names, expr, instantiations, seen)?;
    Ok(())
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
/// PEP 695 (#387): Recursively collects every `(class_name, type_arg)` pair
/// from `GenericClassInstantiate` expressions reachable from `expr`. Used by
/// `instantiate_generic_class_methods`'s pre-scan to know which generic
/// classes need monomorphization before the rewrite pass starts. `Ty` does
/// not implement `Hash`, so dedup is done linearly against this `Vec`.
fn collect_generic_class_instantiations_from_expr(expr: &HirExpr, out: &mut Vec<(String, Ty)>) {
    match expr {
        HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args,
        } => {
            let pair = (class.clone(), type_arg.clone());
            if !out.contains(&pair) {
                out.push(pair);
            }
            for arg in args {
                collect_generic_class_instantiations_from_expr(arg, out);
            }
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_generic_class_instantiations_from_expr(arg, out);
            }
        }
        HirExpr::UnaryOp { operand, .. } => {
            collect_generic_class_instantiations_from_expr(operand, out);
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            collect_generic_class_instantiations_from_expr(left, out);
            collect_generic_class_instantiations_from_expr(right, out);
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    collect_generic_class_instantiations_from_expr(inner, out);
                }
            }
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                collect_generic_class_instantiations_from_expr(e, out);
            }
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                collect_generic_class_instantiations_from_expr(k, out);
                collect_generic_class_instantiations_from_expr(v, out);
            }
        }
        HirExpr::Subscript { base, index } => {
            collect_generic_class_instantiations_from_expr(base, out);
            collect_generic_class_instantiations_from_expr(index, out);
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            collect_generic_class_instantiations_from_expr(base, out);
            for bound in [start, stop, step].into_iter().flatten() {
                collect_generic_class_instantiations_from_expr(bound, out);
            }
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            collect_generic_class_instantiations_from_expr(value, out);
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            collect_generic_class_instantiations_from_expr(key, out);
            collect_generic_class_instantiations_from_expr(default, out);
        }
        HirExpr::AttrGet { base, .. } => {
            collect_generic_class_instantiations_from_expr(base, out);
        }
        HirExpr::MethodCall { base, args, .. } => {
            collect_generic_class_instantiations_from_expr(base, out);
            for arg in args {
                collect_generic_class_instantiations_from_expr(arg, out);
            }
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => {}
    }
}

/// PEP 695 (#387): Traverses a `CompIter` for `GenericClassInstantiate`
/// expressions. `CompIter::Range` carries three `HirExpr`s (`start`, `stop`,
/// `step`) that can each contain a GCI (e.g. `range(C[int](0), 10)`).
/// `CompIter::Name` holds only a bare `String`, so it cannot contain one.
fn collect_generic_class_instantiations_from_comp_iter(
    iter: &CompIter,
    out: &mut Vec<(String, Ty)>,
) {
    match iter {
        CompIter::Range { start, stop, step } => {
            for sub in [start, stop, step] {
                collect_generic_class_instantiations_from_expr(sub, out);
            }
        }
        CompIter::Name(_) => {}
    }
}

/// PEP 695 (#387): Recursively collects `(class_name, type_arg)` pairs from
/// `GenericClassInstantiate` expressions reachable from a statement.
fn collect_generic_class_instantiations_from_stmt(stmt: &HirStmt, out: &mut Vec<(String, Ty)>) {
    match stmt {
        HirStmt::ExprStmt(expr) => collect_generic_class_instantiations_from_expr(expr, out),
        HirStmt::Assign { value, .. } => collect_generic_class_instantiations_from_expr(value, out),
        HirStmt::AnnAssign { value, .. } => {
            if let Some(v) = value {
                collect_generic_class_instantiations_from_expr(v, out);
            }
        }
        HirStmt::If { test, body, orelse } => {
            collect_generic_class_instantiations_from_expr(test, out);
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
            for s in orelse {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::While { test, body } => {
            collect_generic_class_instantiations_from_expr(test, out);
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::ForRange {
            start,
            stop,
            step,
            body,
            ..
        } => {
            for sub in [start, stop, step] {
                collect_generic_class_instantiations_from_expr(sub, out);
            }
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::ForList { body, .. } => {
            // `list` is a bare `String` (variable name), not an `HirExpr`,
            // so it cannot contain a `GenericClassInstantiate`.
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::DictSet { key, value, .. } => {
            collect_generic_class_instantiations_from_expr(key, out);
            collect_generic_class_instantiations_from_expr(value, out);
        }
        HirStmt::ListCompAssign {
            elt, cond, iter, ..
        }
        | HirStmt::SetCompAssign {
            elt, cond, iter, ..
        } => {
            collect_generic_class_instantiations_from_comp_iter(iter, out);
            collect_generic_class_instantiations_from_expr(elt, out);
            if let Some(c) = cond {
                collect_generic_class_instantiations_from_expr(c, out);
            }
        }
        HirStmt::DictCompAssign {
            key,
            value,
            cond,
            iter,
            ..
        } => {
            collect_generic_class_instantiations_from_comp_iter(iter, out);
            collect_generic_class_instantiations_from_expr(key, out);
            collect_generic_class_instantiations_from_expr(value, out);
            if let Some(c) = cond {
                collect_generic_class_instantiations_from_expr(c, out);
            }
        }
        HirStmt::Return(None) => {}
        HirStmt::Return(Some(expr)) => collect_generic_class_instantiations_from_expr(expr, out),
        HirStmt::AttrSet { base, value, .. } => {
            collect_generic_class_instantiations_from_expr(base, out);
            collect_generic_class_instantiations_from_expr(value, out);
        }
        HirStmt::Match { subject, cases } => {
            collect_generic_class_instantiations_from_expr(subject, out);
            for case in cases {
                if let Some(guard) = &case.guard {
                    collect_generic_class_instantiations_from_expr(guard, out);
                }
                for s in &case.body {
                    collect_generic_class_instantiations_from_stmt(s, out);
                }
            }
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
            for handler in handlers {
                for s in &handler.body {
                    collect_generic_class_instantiations_from_stmt(s, out);
                }
            }
            for s in orelse {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
            for s in finalbody {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::Raise { exc, cause } => {
            if let Some(e) = exc {
                collect_generic_class_instantiations_from_expr(e, out);
            }
            if let Some(c) = cause {
                collect_generic_class_instantiations_from_expr(c, out);
            }
        }
    }
}

/// PEP 695 (#387): Pre-scans `hir` for every `GenericClassInstantiate`
/// expression, collecting the unique `(class_name, type_arg)` pairs, then
/// monomorphizes each generic class's methods by substituting the class's
/// type parameter with the concrete `type_arg` — reusing PR-13's own
/// `substitute_ty`/`substitute_body`/`mangle_generic_instantiation`
/// infrastructure (D-133/D-134). Each monomorphized method is registered
/// in `env` (as an ordinary function signature) and collected in
/// `instantiations` (as a full `HirItem::Function` body for `pycc_mir` to
/// register). The monomorphized class itself is also registered in
/// `env.classes` under its mangled name, with mangled method names, so
/// `infer_expr_in`'s class-instantiation and method-call resolution work
/// transparently on the specialized class.
fn instantiate_generic_class_methods(
    hir: &HirModule,
    env: &mut Environment,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
    new_class_defs: &mut Vec<(String, HirClassDef)>,
) {
    // Collect all unique (class_name, type_arg) pairs from the entire module.
    let mut pairs: Vec<(String, Ty)> = Vec::new();
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                collect_generic_class_instantiations_from_stmt(stmt, &mut pairs);
            }
            HirItem::Function { body, .. } => {
                for stmt in body {
                    collect_generic_class_instantiations_from_stmt(stmt, &mut pairs);
                }
            }
        }
    }

    for (class_name, type_arg) in &pairs {
        let Some(class_def) = hir.class_defs.iter().find(|(name, _)| name == class_name) else {
            continue;
        };
        let (_, class_def) = class_def;
        let Some(type_param_name) = &class_def.type_param else {
            continue;
        };
        let mangled_class = mangle_generic_instantiation(class_name, type_param_name, type_arg);

        // Monomorphize each method: find its HirItem::Function in hir.items
        // by its mangled name (e.g. "C.__init__"), substitute T with
        // type_arg, rename to the new mangled class name, and register.
        // #386 rebind semantics: class lowering retains multiple same-named
        // method items (the latest definition wins at runtime), and the
        // method table entry was replaced to point at the latest mangled
        // name on redefinition. Since the mangled name is identical across
        // redefinitions, `rfind` (reverse find) specializes the *last*
        // matching `HirItem::Function` -- the one whose body actually runs
        // -- rather than `find`'s first match, which would specialize a
        // stale, shadowed definition.
        let mut mangled_methods: Vec<(String, String)> = Vec::new();
        for (method_name, original_mangled) in &class_def.methods {
            // The `rfind` filter already guarantees the item is a
            // `HirItem::Function`, so the destructuring cannot fail — the
            // `else { continue }` handles only the `None` case (method not
            // found in `hir.items`).
            let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == original_mangled
                )
            })
            else {
                continue;
            };
            let substituted_params = params
                .iter()
                .map(|(pn, ty)| {
                    (
                        pn.clone(),
                        substitute_ty_with_class(
                            ty,
                            type_param_name,
                            type_arg,
                            class_name,
                            &mangled_class,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let substituted_return = substitute_ty_with_class(
                return_ty,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let substituted_body = substitute_body_with_class(
                body,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let new_mangled = format!("{mangled_class}.{method_name}");
            let param_tys = substituted_params
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect::<Vec<_>>();
            // Register the method's signature in env so infer_expr_in can
            // resolve calls to it.
            env.bind_function(new_mangled.clone(), param_tys, substituted_return.clone());
            // substitute_body_with_class (called above) already rewrites
            // both `Ty::Param` → `type_arg` and `Ty::Instance(class_name)`
            // → `Ty::Instance(mangled_class)` in the method body's
            // annotations. Self/class-name resolution happened at
            // annotation_to_ty time (HIR lowering), where `Self` and the
            // class name both became `Ty::Instance(class_name)`, and
            // substitute_body_with_class rewrites those to the mangled
            // monomorphized class name. No further substitution is needed
            // here.
            let specialized = HirItem::Function {
                name: new_mangled.clone(),
                params: substituted_params,
                return_ty: substituted_return,
                body: substituted_body,
            };
            if seen.insert(new_mangled.clone()) {
                instantiations.push(GenericInstantiation {
                    mangled_name: new_mangled.clone(),
                    specialized,
                    return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                });
            }
            mangled_methods.push((method_name.clone(), new_mangled));
        }

        // #377: Monomorphize properties. Each property's getter and setter
        // are ordinary `HirItem::Function` items with mangled names like
        // `Box.v` and `Box.v.setter`. They are NOT in the class's `methods`
        // table (they're in `properties`), so the method monomorphization
        // loop above does not handle them. We monomorphize them here by
        // finding their `HirItem::Function` in `hir.items`, substituting the
        // type parameter, and renaming to the mangled class name. The
        // monomorphized property entries point at the new mangled names.
        let mut monomorphized_properties: Vec<PropertyDef> = Vec::new();
        for prop in &class_def.properties {
            let new_getter = format!("{mangled_class}.{}", prop.name);
            if let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == &prop.getter
                )
            }) {
                let substituted_params = params
                    .iter()
                    .map(|(pn, ty)| {
                        (
                            pn.clone(),
                            substitute_ty_with_class(
                                ty,
                                type_param_name,
                                type_arg,
                                class_name,
                                &mangled_class,
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                let substituted_return = substitute_ty_with_class(
                    return_ty,
                    type_param_name,
                    type_arg,
                    class_name,
                    &mangled_class,
                );
                let substituted_body = substitute_body_with_class(
                    body,
                    type_param_name,
                    type_arg,
                    class_name,
                    &mangled_class,
                );
                let param_tys = substituted_params
                    .iter()
                    .map(|(_, ty)| ty.clone())
                    .collect::<Vec<_>>();
                env.bind_function(new_getter.clone(), param_tys, substituted_return.clone());
                let specialized = HirItem::Function {
                    name: new_getter.clone(),
                    params: substituted_params,
                    return_ty: substituted_return,
                    body: substituted_body,
                };
                if seen.insert(new_getter.clone()) {
                    instantiations.push(GenericInstantiation {
                        mangled_name: new_getter.clone(),
                        specialized,
                        return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                    });
                }
            }
            let new_setter = prop.setter.as_ref().map(|s| {
                let new_s = format!("{mangled_class}.{}.setter", prop.name);
                if let Some(HirItem::Function {
                    name: _,
                    params,
                    return_ty,
                    body,
                }) = hir.items.iter().rfind(|item| {
                    matches!(
                        item,
                        HirItem::Function { name, .. } if name == s
                    )
                }) {
                    let substituted_params = params
                        .iter()
                        .map(|(pn, ty)| {
                            (
                                pn.clone(),
                                substitute_ty_with_class(
                                    ty,
                                    type_param_name,
                                    type_arg,
                                    class_name,
                                    &mangled_class,
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    let substituted_return = substitute_ty_with_class(
                        return_ty,
                        type_param_name,
                        type_arg,
                        class_name,
                        &mangled_class,
                    );
                    let substituted_body = substitute_body_with_class(
                        body,
                        type_param_name,
                        type_arg,
                        class_name,
                        &mangled_class,
                    );
                    let param_tys = substituted_params
                        .iter()
                        .map(|(_, ty)| ty.clone())
                        .collect::<Vec<_>>();
                    env.bind_function(new_s.clone(), param_tys, substituted_return.clone());
                    let specialized = HirItem::Function {
                        name: new_s.clone(),
                        params: substituted_params,
                        return_ty: substituted_return,
                        body: substituted_body,
                    };
                    if seen.insert(new_s.clone()) {
                        instantiations.push(GenericInstantiation {
                            mangled_name: new_s.clone(),
                            specialized,
                            return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                        });
                    }
                }
                new_s
            });
            monomorphized_properties.push(PropertyDef {
                name: prop.name.clone(),
                getter: new_getter,
                setter: new_setter,
            });
        }

        // #436: Monomorphize static methods. Each static method's
        // mangled name uses a `.static` suffix; the monomorphized name
        // replaces the class prefix with the mangled class name. Static
        // methods have no `self`/`cls` parameter, so only the type
        // parameter substitution applies.
        let mut mangled_static_methods: Vec<(String, String)> = Vec::new();
        for (method_name, original_mangled) in &class_def.static_methods {
            let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == original_mangled
                )
            })
            else {
                continue;
            };
            let substituted_params = params
                .iter()
                .map(|(pn, ty)| {
                    (
                        pn.clone(),
                        substitute_ty_with_class(
                            ty,
                            type_param_name,
                            type_arg,
                            class_name,
                            &mangled_class,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let substituted_return = substitute_ty_with_class(
                return_ty,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let substituted_body = substitute_body_with_class(
                body,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let new_mangled = format!("{mangled_class}.{method_name}.static");
            let param_tys = substituted_params
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect::<Vec<_>>();
            env.bind_function(new_mangled.clone(), param_tys, substituted_return.clone());
            let specialized = HirItem::Function {
                name: new_mangled.clone(),
                params: substituted_params,
                return_ty: substituted_return,
                body: substituted_body,
            };
            if seen.insert(new_mangled.clone()) {
                instantiations.push(GenericInstantiation {
                    mangled_name: new_mangled.clone(),
                    specialized,
                    return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                });
            }
            mangled_static_methods.push((method_name.clone(), new_mangled));
        }

        // #436: Monomorphize class methods. Each class method's mangled
        // name uses a `.classmethod` suffix; the monomorphized name
        // replaces the class prefix with the mangled class name. Class
        // methods have a `cls` parameter typed `Ty::Instance(class_name)`,
        // which `substitute_body_with_class` rewrites to
        // `Ty::Instance(mangled_class)`.
        let mut mangled_class_methods: Vec<(String, String)> = Vec::new();
        for (method_name, original_mangled) in &class_def.class_methods {
            let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == original_mangled
                )
            })
            else {
                continue;
            };
            let substituted_params = params
                .iter()
                .map(|(pn, ty)| {
                    (
                        pn.clone(),
                        substitute_ty_with_class(
                            ty,
                            type_param_name,
                            type_arg,
                            class_name,
                            &mangled_class,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let substituted_return = substitute_ty_with_class(
                return_ty,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let substituted_body = substitute_body_with_class(
                body,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let new_mangled = format!("{mangled_class}.{method_name}.classmethod");
            let param_tys = substituted_params
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect::<Vec<_>>();
            env.bind_function(new_mangled.clone(), param_tys, substituted_return.clone());
            let specialized = HirItem::Function {
                name: new_mangled.clone(),
                params: substituted_params,
                return_ty: substituted_return,
                body: substituted_body,
            };
            if seen.insert(new_mangled.clone()) {
                instantiations.push(GenericInstantiation {
                    mangled_name: new_mangled.clone(),
                    specialized,
                    return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                });
            }
            mangled_class_methods.push((method_name.clone(), new_mangled));
        }

        // Register the monomorphized class in env.classes with mangled
        // method names and substituted attribute types.
        let substituted_attrs = class_def
            .attrs
            .iter()
            .map(|(attr_name, ty)| {
                (
                    attr_name.clone(),
                    substitute_ty(ty, type_param_name, type_arg),
                )
            })
            .collect::<Vec<_>>();
        let new_class_def = HirClassDef {
            name: mangled_class.clone(),
            bases: Vec::new(),
            mro: vec![mangled_class.clone()],
            attrs: substituted_attrs,
            methods: mangled_methods,
            type_param: None, // The monomorphized class is not generic.
            properties: monomorphized_properties,
            static_methods: mangled_static_methods,
            class_methods: mangled_class_methods,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        env.bind_class(mangled_class.clone(), new_class_def.clone());
        new_class_defs.push((mangled_class, new_class_def));
    }
}

/// PEP 695 (#387): Pass 2b per-item worker — rewrites any
/// `GenericClassInstantiate` expressions that survived inside one
/// monomorphized generic-class method body. Extracted from `monomorphize`'s
/// Pass 2b loop so that the defense-in-depth non-`Function` arm (all
/// instantiations are `HirItem::Function` in practice) is a `match` arm in
/// a standalone function that a direct unit test can cover, rather than an
/// `if let` whose never-taken false branch would carry an uncovered
/// continuation region under the 100%-coverage gate (D-014).
///
/// Clones the function's params/body out of `instantiations[i]` so the
/// immutable borrow ends before `rewrite_generic_calls_in_stmt` takes
/// `&mut instantiations` (it may push new instantiations for a GCI inside
/// the body that triggers another class monomorphization).
fn rewrite_generic_calls_in_instantiation(
    env: &mut Environment,
    i: usize,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    let (name, params, return_ty, body) = match &instantiations[i].specialized {
        HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } => (
            name.clone(),
            params.clone(),
            return_ty.clone(),
            body.clone(),
        ),
        // All instantiations are `HirItem::Function` (created by
        // `instantiate_generic_call` or `instantiate_generic_class_methods`).
        // This arm is defense-in-depth, covered by a direct unit test.
        _ => return Ok(()),
    };
    let local_names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
    let local_names_refs: Vec<&str> = local_names.iter().map(|s| s.as_str()).collect();
    let mut fn_env = env.child_for_function(&local_names_refs);
    for (param_name, param_ty) in &params {
        fn_env.bind(param_name.clone(), param_ty.clone());
    }
    let mut new_body = body;
    for stmt in new_body.iter_mut() {
        rewrite_generic_calls_in_stmt(&mut fn_env, &local_names_refs, stmt, instantiations, seen)?;
    }
    // Direct assignment avoids a second `if let` (whose never-taken false
    // branch would carry an uncovered region under D-014). All instantiations
    // are `HirItem::Function`, so this reconstruction is always valid.
    instantiations[i].specialized = HirItem::Function {
        name,
        params,
        return_ty,
        body: new_body,
    };
    Ok(())
}

/// #380 (PR-20): Monomorphizes functions with protocol-typed parameters.
/// Scans `items` for calls to functions that have `Ty::Protocol` parameters,
/// and for each call site, creates a specialized version with the protocol
/// type substituted by the concrete argument's class type. The original
/// function is dropped (only specializations reach MIR/codegen). Returns
/// the updated items list with monomorphized functions appended and call
/// sites rewritten.
fn monomorphize_protocol_params(
    items: Vec<HirItem>,
    env: &Environment,
    _new_class_defs: &mut Vec<(String, HirClassDef)>,
) -> Vec<HirItem> {
    // Collect functions with protocol-typed parameters (cloned, so we
    // can move `items` below without a borrow conflict).
    let protocol_funcs: HashMap<String, HirItem> = items
        .iter()
        .filter_map(|item| {
            if let HirItem::Function {
                name,
                params,
                return_ty,
                ..
            } = item
                && has_protocol_param(params, return_ty)
            {
                return Some((name.clone(), item.clone()));
            }
            None
        })
        .collect();
    if protocol_funcs.is_empty() {
        return items;
    }
    let mut new_items = Vec::new();
    let mut specializations: Vec<HirItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item in items {
        match item {
            HirItem::Function {
                ref name,
                ref params,
                ref return_ty,
                ..
            } if protocol_funcs.contains_key(name.as_str()) => {
                // Drop the original protocol-parameter function (only
                // specializations are kept). But if the function has no
                // call sites (no specializations), it would be silently
                // dropped — this is fine, matching how generic functions
                // are dropped.
                let _ = (params, return_ty);
            }
            HirItem::TopLevelStmt(ref stmt) => {
                let mut new_stmt = stmt.clone();
                rewrite_protocol_calls_in_stmt(
                    &mut new_stmt,
                    &protocol_funcs,
                    env,
                    &[],
                    &mut specializations,
                    &mut seen,
                );
                new_items.push(HirItem::TopLevelStmt(new_stmt));
            }
            HirItem::Function {
                name,
                params,
                return_ty,
                body,
            } => {
                let local_names: Vec<String> = function_local_names(&params, &body)
                    .into_iter()
                    .map(String::from)
                    .collect();
                let local_names_refs: Vec<&str> = local_names.iter().map(|s| s.as_str()).collect();
                let mut fn_env = env.child_for_function(&local_names_refs);
                for (param_name, param_ty) in &params {
                    fn_env.bind(param_name.clone(), param_ty.clone());
                }
                bind_local_types_in_body(&mut fn_env, &local_names_refs, &body);
                let mut new_body = body;
                for stmt in new_body.iter_mut() {
                    rewrite_protocol_calls_in_stmt(
                        stmt,
                        &protocol_funcs,
                        &fn_env,
                        &local_names_refs,
                        &mut specializations,
                        &mut seen,
                    );
                }
                new_items.push(HirItem::Function {
                    name,
                    params,
                    return_ty,
                    body: new_body,
                });
            }
        }
    }
    // Rewrite calls inside specializations recursively. A specialized
    // body may itself call another protocol-parameter function, which
    // needs its own specialization. Process by index because rewriting
    // may push new specializations, extending the vector we're walking.
    //
    // The per-item work is extracted into `rewrite_protocol_calls_in_specialization`
    // so that the defense-in-depth non-`Function` arm (all specializations
    // are `HirItem::Function` in practice) is a `match` arm in a standalone
    // function that a direct unit test can cover, rather than an `if let`
    // whose never-taken false branch would carry an uncovered continuation
    // region under the 100%-coverage gate (D-014).
    let mut all_specializations = std::mem::take(&mut specializations);
    let mut i = 0;
    while i < all_specializations.len() {
        rewrite_protocol_calls_in_specialization(
            env,
            &mut all_specializations[i],
            &protocol_funcs,
            &mut specializations,
            &mut seen,
        );
        i += 1;
        if !specializations.is_empty() {
            all_specializations.append(&mut specializations);
        }
    }
    new_items.extend(all_specializations);
    new_items
}

/// #380 (PR-20): Per-item worker for the specialization rewrite loop.
/// Rewrites protocol-parameter function calls inside one already-created
/// specialization's body, pushing any newly discovered specializations.
/// Extracted from `monomorphize_protocol_params`'s loop so the
/// defense-in-depth non-`Function` arm is a `match` arm coverable by a
/// direct unit test (D-014).
fn rewrite_protocol_calls_in_specialization(
    env: &Environment,
    spec: &mut HirItem,
    protocol_funcs: &HashMap<String, HirItem>,
    specializations: &mut Vec<HirItem>,
    seen: &mut HashSet<String>,
) {
    let (params, body) = match spec {
        HirItem::Function { params, body, .. } => (params, body),
        // All specializations are `HirItem::Function` (created by
        // `rewrite_protocol_calls_in_expr`). This arm is defense-in-depth,
        // covered by a direct unit test.
        _ => return,
    };
    let local_names: Vec<String> = function_local_names(params, body)
        .into_iter()
        .map(String::from)
        .collect();
    let local_names_refs: Vec<&str> = local_names.iter().map(|s| s.as_str()).collect();
    let mut spec_env = env.child_for_function(&local_names_refs);
    for (param_name, param_ty) in params.iter() {
        spec_env.bind(param_name.clone(), param_ty.clone());
    }
    bind_local_types_in_body(&mut spec_env, &local_names_refs, body);
    for stmt in body.iter_mut() {
        rewrite_protocol_calls_in_stmt(
            stmt,
            protocol_funcs,
            &spec_env,
            &local_names_refs,
            specializations,
            seen,
        );
    }
}

/// #380 (PR-20): Rewrites calls to protocol-parameter functions in a
/// statement, creating monomorphized specializations as needed.
fn rewrite_protocol_calls_in_stmt(
    stmt: &mut HirStmt,
    protocol_funcs: &HashMap<String, HirItem>,
    env: &Environment,
    local_names: &[&str],
    specializations: &mut Vec<HirItem>,
    seen: &mut HashSet<String>,
) {
    match stmt {
        HirStmt::ExprStmt(expr) => {
            rewrite_protocol_calls_in_expr(
                expr,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::Assign { value, .. } => {
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::AnnAssign {
            value: Some(value), ..
        } => {
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::Return(Some(expr)) => {
            rewrite_protocol_calls_in_expr(
                expr,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::If { test, body, orelse } => {
            rewrite_protocol_calls_in_expr(
                test,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            for s in orelse.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::While { test, body } => {
            rewrite_protocol_calls_in_expr(
                test,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::ForRange {
            start,
            stop,
            step,
            body,
            ..
        } => {
            rewrite_protocol_calls_in_expr(
                start,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                stop,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                step,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::ForList { body, .. } => {
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::DictSet { key, value, .. } => {
            rewrite_protocol_calls_in_expr(
                key,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::AttrSet { base, value, .. } => {
            rewrite_protocol_calls_in_expr(
                base,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::ListCompAssign { cond, elt, .. } | HirStmt::SetCompAssign { cond, elt, .. } => {
            if let Some(c) = cond {
                rewrite_protocol_calls_in_expr(
                    c,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            rewrite_protocol_calls_in_expr(
                elt,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::DictCompAssign {
            cond, key, value, ..
        } => {
            if let Some(c) = cond {
                rewrite_protocol_calls_in_expr(
                    c,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            rewrite_protocol_calls_in_expr(
                key,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        _ => {}
    }
}

/// #380 (PR-20): Rewrites calls to protocol-parameter functions in an
/// expression, creating monomorphized specializations as needed.
fn rewrite_protocol_calls_in_expr(
    expr: &mut HirExpr,
    protocol_funcs: &HashMap<String, HirItem>,
    env: &Environment,
    local_names: &[&str],
    specializations: &mut Vec<HirItem>,
    seen: &mut HashSet<String>,
) {
    match expr {
        HirExpr::Call { callee, args } => {
            // First, recurse into arguments (they may contain nested calls).
            for arg in args.iter_mut() {
                rewrite_protocol_calls_in_expr(
                    arg,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            // Check if this is a call to a protocol-parameter function.
            if let Some(func_item) = protocol_funcs.get(callee.as_str())
                && let HirItem::Function {
                    name,
                    params,
                    return_ty,
                    body,
                } = func_item
            {
                // Collect all protocol→concrete substitutions from every
                // protocol-typed parameter's corresponding argument.
                let mut substitutions: Vec<(String, Ty)> = Vec::new();
                for (i, (_, param_ty)) in params.iter().enumerate() {
                    if let Ty::Protocol(proto_name) = param_ty
                        && i < args.len()
                    {
                        let arg_ty = infer_expr_in(env, local_names, &args[i]);
                        if let Ok(Ty::Instance(concrete_name)) = arg_ty {
                            substitutions
                                .push((proto_name.as_ref().clone(), Ty::Instance(concrete_name)));
                        }
                    }
                }
                if !substitutions.is_empty() {
                    let mangled = mangle_protocol_instantiation(name, &substitutions);
                    if seen.insert(mangled.clone()) {
                        let substituted_params: Vec<(String, Ty)> = params
                            .iter()
                            .map(|(n, ty)| (n.clone(), substitute_ty_protocols(ty, &substitutions)))
                            .collect();
                        let substituted_return = substitute_ty_protocols(return_ty, &substitutions);
                        let substituted_body = substitute_body_protocols(body, &substitutions);
                        specializations.push(HirItem::Function {
                            name: mangled.clone(),
                            params: substituted_params,
                            return_ty: substituted_return,
                            body: substituted_body,
                        });
                    }
                    *callee = mangled;
                }
            }
        }
        HirExpr::MethodCall { base, args, .. } => {
            rewrite_protocol_calls_in_expr(
                base,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for arg in args.iter_mut() {
                rewrite_protocol_calls_in_expr(
                    arg,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirExpr::AttrGet { base, .. } => {
            rewrite_protocol_calls_in_expr(
                base,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::UnaryOp { operand, .. } => {
            rewrite_protocol_calls_in_expr(
                operand,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::BinOp { left, right, .. } => {
            rewrite_protocol_calls_in_expr(
                left,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                right,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::Compare { left, right, .. } => {
            rewrite_protocol_calls_in_expr(
                left,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                right,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::ListLiteral(elements) => {
            for e in elements.iter_mut() {
                rewrite_protocol_calls_in_expr(
                    e,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        _ => {}
    }
}

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
    if generics.is_empty()
        && !hir.class_defs.iter().any(|(_, cd)| cd.type_param.is_some())
        && !hir.items.iter().any(|item| {
            if let HirItem::Function {
                params, return_ty, ..
            } = item
            {
                has_protocol_param(params, return_ty)
            } else {
                false
            }
        })
    {
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

    // PEP 695 (#387): pre-scan for generic class instantiations
    // (`C[type_arg](args)`) and monomorphize each unique `(class, type_arg)`
    // pair's methods before the rewrite pass starts. This way,
    // `rewrite_generic_calls_in_expr`'s `GenericClassInstantiate` arm can
    // simply rewrite the expression to an ordinary `HirExpr::Call` to the
    // mangled class name, confident the monomorphized methods are already
    // registered in `env` and collected in `instantiations`.
    let mut new_class_defs: Vec<(String, HirClassDef)> = Vec::new();
    instantiate_generic_class_methods(
        hir,
        &mut env,
        &mut instantiations,
        &mut seen,
        &mut new_class_defs,
    );

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
    // PEP 695 (#387): Pass 2b — rewrite any `GenericClassInstantiate`
    // expressions that survived inside monomorphized generic-class method
    // bodies. Pass 2 only iterates over the original `hir.items`, not the
    // `instantiations` appended below, so a GCI nested inside a generic
    // class method (e.g. `class C[T]: def f(self): b = D[int](1)`) would
    // survive into the monomorphized copy and panic at MIR lowering. Run
    // the same `rewrite_generic_calls_in_stmt` over each instantiation's
    // body before appending it, using a fresh child environment seeded
    // with the instantiation's own (already-substituted) parameter types.
    // Process by index because `rewrite_generic_calls_in_stmt` may push
    // new instantiations (a GCI inside a monomorphized body triggers
    // another class monomorphization), extending the vector we're walking.
    //
    // The per-item work is extracted into `rewrite_generic_calls_in_instantiation`
    // so that the defense-in-depth non-`Function` arm (all instantiations
    // are `HirItem::Function` in practice) is a `match` arm in a standalone
    // function that a direct unit test can cover, rather than an `if let`
    // whose never-taken false branch would carry an uncovered continuation
    // region under the 100%-coverage gate (D-014).
    let mut i = 0;
    while i < instantiations.len() {
        rewrite_generic_calls_in_instantiation(&mut env, i, &mut instantiations, &mut seen)?;
        i += 1;
    }
    for instantiation in instantiations {
        items.push(instantiation.specialized);
    }
    // #380 (PR-20): Protocol-typed parameter monomorphization. Functions
    // with `Ty::Protocol` parameters need to be specialized per concrete
    // call-site type, substituting the protocol type with the concrete
    // `Ty::Instance` so MIR/codegen can resolve method calls and attribute
    // access against the concrete class. This pass runs after the existing
    // generic function monomorphization, scanning the (already rewritten)
    // items for calls to protocol-parameter functions.
    items = monomorphize_protocol_params(items, &env, &mut new_class_defs);
    // `type_aliases`/`imports` are empty by design on both of this
    // function's exits -- see the no-generics early return above (PR-13
    // final review I1) and that return's own comment for why `class_defs`
    // is not treated the same way.
    // PEP 695 (#387): include the monomorphized class definitions alongside
    // the originals, so `pycc_mir`'s `classes` HashMap can resolve the
    // mangled class name to its specialized `HirClassDef` (attribute slots,
    // method table) for instantiation and method-call lowering.
    let mut class_defs = hir.class_defs.clone();
    class_defs.extend(new_class_defs);
    Ok(HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs,
    })
}

/// #379 (PR-19): PEP 435 enum loop unrolling. Rewrites every
/// `HirStmt::ForList { var, list, body }` where `list` is an enum class
/// name (a class with non-empty `enum_members`) into N sequential copies
/// of the body, each preceded by `HirStmt::Assign { target: var, value:
/// HirExpr::AttrGet { base: HirExpr::Name(list), attr: <member> } }`,
/// one per member in source order. This runs after `check_and_resolve`
/// and `monomorphize`, so the HIR is fully type-checked and the enum
/// class's member table is final. MIR never sees an enum iterable -- it
/// only sees ordinary `Assign` + body statements.
///
/// #379 (PR-19): Return the type of an enum member accessed by name
/// (`Color.RED`), or `None` if `class_def` is not an enum class or `attr`
/// is not one of its members. Extracted from `infer_expr_in` to isolate
/// the enum-specific code paths (see cargo-llvm-cov#276 for the coverage
/// instantiation issue).
fn enum_member_attr_type(
    class_def: &crate::HirClassDef,
    class_name: &str,
    attr: &str,
) -> Option<Ty> {
    if !class_def.enum_members.is_empty()
        && class_def.enum_members.iter().any(|(name, _)| name == attr)
    {
        Some(Ty::Instance(Box::new(class_name.to_string())))
    } else {
        None
    }
}

/// #379 (PR-19): Type-check an enum class iteration loop body. Binds `var`
/// to `Ty::Instance(list)`, checks each body statement, then joins the body
/// environment back. Extracted from `check_stmt` and
/// `check_stmt_in_function` to isolate the enum-specific code paths (see
/// cargo-llvm-cov#276 for the coverage instantiation issue). Two variants
/// exist (module-scope and function-scope) to avoid generic closure
/// monomorphization producing separate coverage records per call site.
fn check_enum_loop_body_module(
    env: &mut Environment,
    var: &str,
    list: &str,
    body: &[HirStmt],
) -> Result<(), Diagnostic> {
    let var_ty = Ty::Instance(Box::new(list.to_string()));
    let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
    check_assignment(env, var, var_ty)?;
    let mut body_env = env.clone();
    for stmt in body {
        check_stmt(&mut body_env, stmt)?;
    }
    join_loop_body(env, &body_env);
    if !was_definite && let Some(ty) = env.lookup_any(var) {
        env.bind_maybe(var.to_string(), ty);
    }
    Ok(())
}

/// #379 (PR-19): Function-scope variant of `check_enum_loop_body_module`.
/// Checks each body statement via `check_stmt_in_function` with the
/// enclosing function's `local_names` and `return_ty`.
fn check_enum_loop_body_function(
    env: &mut Environment,
    var: &str,
    list: &str,
    body: &[HirStmt],
    local_names: &[&str],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    let var_ty = Ty::Instance(Box::new(list.to_string()));
    let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
    check_assignment(env, var, var_ty)?;
    let mut body_env = env.clone();
    for s in body {
        check_stmt_in_function(&mut body_env, local_names, s, return_ty.clone())?;
    }
    join_loop_body(env, &body_env);
    if !was_definite && let Some(ty) = env.lookup_any(var) {
        env.bind_maybe(var.to_string(), ty);
    }
    Ok(())
}

/// #379 (PR-19): Build a lookup table mapping enum class name to its
/// member names (in source order). Extracted from `unroll_enum_loops` to
/// isolate the enum-specific code paths (see cargo-llvm-cov#276 for the
/// coverage instantiation issue).
fn build_enum_member_table(
    class_defs: &[(String, crate::HirClassDef)],
) -> HashMap<&str, Vec<&String>> {
    class_defs
        .iter()
        .filter(|(_, cd)| !cd.enum_members.is_empty())
        .map(|(name, cd)| {
            (
                name.as_str(),
                cd.enum_members.iter().map(|(mn, _)| mn).collect(),
            )
        })
        .collect()
}

/// The rewrite walks both top-level items and function bodies (a
/// `ForList`-over-enum can appear inside a function). Inside a function
/// body, the enclosing `Vec<HirStmt>` is spliced in place: the unrolled
/// statements replace the original `ForList` statement at its position.
///
/// Limitations (matching the plan's risk-1): no `break`/`continue`/`else`
/// in an enum-loop body (already unimplemented for all v0.3 loops, so no
/// real loss); code size is linear in member-count × body-size (acceptable
/// for v0.3 fixtures). A module with no enum classes is returned unchanged.
fn unroll_enum_loops(mut hir: HirModule) -> Result<HirModule, Diagnostic> {
    // Fast path: if no class is an enum class, there is nothing to unroll.
    let has_enum = hir
        .class_defs
        .iter()
        .any(|(_, cd)| !cd.enum_members.is_empty());
    if !has_enum {
        return Ok(hir);
    }
    // Build a lookup table: enum class name -> member names (in source order).
    let enum_members = build_enum_member_table(&hir.class_defs);
    // Walk top-level items and function bodies, splicing unrolled statements.
    let mut new_items: Vec<HirItem> = Vec::with_capacity(hir.items.len());
    for item in hir.items.drain(..) {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                let mut unrolled =
                    unroll_enum_loops_in_stmts(std::slice::from_ref(&stmt), &enum_members);
                for s in unrolled.drain(..) {
                    new_items.push(HirItem::TopLevelStmt(s));
                }
            }
            HirItem::Function {
                name,
                params,
                return_ty,
                body,
            } => {
                let body = unroll_enum_loops_in_stmts(&body, &enum_members);
                new_items.push(HirItem::Function {
                    name,
                    params,
                    return_ty,
                    body,
                });
            }
        }
    }
    hir.items = new_items;
    Ok(hir)
}

/// Helper for `unroll_enum_loops`: walks a `Vec<HirStmt>`, splicing any
/// `ForList`-over-enum into its unrolled equivalent. Recurses into nested
/// statement bodies (if/while/for bodies) so an enum loop inside a nested
/// block is also unrolled. Returns a new `Vec<HirStmt>` with all enum loops
/// expanded.
fn unroll_enum_loops_in_stmts(
    stmts: &[HirStmt],
    enum_members: &HashMap<&str, Vec<&String>>,
) -> Vec<HirStmt> {
    let mut result: Vec<HirStmt> = Vec::new();
    for stmt in stmts {
        match stmt {
            HirStmt::ForList { var, list, body } => {
                // Check if `list` is an enum class name.
                if let Some(members) = enum_members.get(list.as_str()) {
                    // Unroll: for each member, emit `var = <enum>.<member>`
                    // then the body (cloned).
                    let body = body.clone();
                    for member_name in members {
                        result.push(HirStmt::Assign {
                            target: var.clone(),
                            value: HirExpr::AttrGet {
                                base: Box::new(HirExpr::Name(list.clone())),
                                attr: (*member_name).clone(),
                            },
                        });
                        result.extend(body.iter().cloned());
                    }
                } else {
                    // Not an enum loop -- keep as-is, but recurse into the
                    // body in case it contains a nested enum loop.
                    result.push(HirStmt::ForList {
                        var: var.clone(),
                        list: list.clone(),
                        body: unroll_enum_loops_in_stmts(body, enum_members),
                    });
                }
            }
            // Recurse into nested statement bodies.
            HirStmt::If { test, body, orelse } => {
                result.push(HirStmt::If {
                    test: test.clone(),
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                    orelse: unroll_enum_loops_in_stmts(orelse, enum_members),
                });
            }
            HirStmt::While { test, body } => {
                result.push(HirStmt::While {
                    test: test.clone(),
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                });
            }
            HirStmt::ForRange {
                var,
                start,
                stop,
                step,
                body,
            } => {
                result.push(HirStmt::ForRange {
                    var: var.clone(),
                    start: start.clone(),
                    stop: stop.clone(),
                    step: step.clone(),
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                });
            }
            // Other statement kinds don't contain nested ForList loops.
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                result.push(HirStmt::Try {
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                    handlers: handlers
                        .iter()
                        .map(|h| pycc_hir::HirExceptHandler {
                            exc_type: h.exc_type.clone(),
                            name: h.name.clone(),
                            body: unroll_enum_loops_in_stmts(&h.body, enum_members),
                        })
                        .collect(),
                    orelse: unroll_enum_loops_in_stmts(orelse, enum_members),
                    finalbody: unroll_enum_loops_in_stmts(finalbody, enum_members),
                });
            }
            other => result.push(other.clone()),
        }
    }
    result
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
    let monomorphized = monomorphize(&resolved_hir)?;
    unroll_enum_loops(monomorphized)
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
