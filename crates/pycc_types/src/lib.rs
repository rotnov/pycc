mod binop;
mod class;
mod constraints;
mod enum_lower;
mod env;
mod exception;
mod expr;
mod monomorphize;
mod narrow;
mod solver;
#[cfg(test)]
mod tests;
mod unop;

pub(crate) use enum_lower::{
    check_enum_loop_body_function, check_enum_loop_body_module, enum_member_attr_type,
    unroll_enum_loops,
};
pub(crate) use env::BindingState;
pub use env::Environment;
use exception::{
    check_raise_stmt, check_try_star_stmt, check_try_stmt, is_unshadowed_builtin_exception,
};
pub use expr::infer_expr;
pub(crate) use expr::infer_expr_in;

pub(crate) use constraints::*;
pub use monomorphize::*;

use pycc_diag::{Diagnostic, Span};
#[cfg(test)]
use pycc_hir::BinOpKind;
#[cfg(test)]
use pycc_hir::CmpOpKind;
#[cfg(test)]
use pycc_hir::PropertyDef;
#[cfg(test)]
use pycc_hir::UnaryOpKind;
pub use pycc_hir::Ty;
use pycc_hir::{
    CompIter, FStringPart, HirClassDef, HirExpr, HirItem, HirMatchCase, HirModule, HirPattern,
    HirStmt,
};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Arc;

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

/// An annotation-subscript marker (`typing.Final`, `typing.Annotated`,
/// #762) referenced as a first-class value or called directly, instead of
/// being used as an annotation subscript (`Final[int]`, `Annotated[int,
/// ...]`). Unlike the other marker kinds, `Final`/`Annotated` are never
/// valid as a base class or a decorator, so `marker_is_not_a_value`'s
/// generic guidance would be misleading here (review finding on PR #766).
fn annotation_marker_is_not_a_value(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "`{name}` is an annotation marker, not a first-class value — use it only as an annotation subscript (e.g. `{name}[int]`)"
        ),
        Span::new(0, 0),
    )
}

/// The `typing.cast` marker (#767) referenced as a first-class value, or
/// called through its qualified name (`typing.cast(int, x)`) instead of the
/// bare name imported with `from typing import cast`. `cast` is recognized
/// by bare callee name in `infer_expr_in`/`collect_expr_constraints` (like
/// `isinstance`/`issubclass`), so neither the qualified call form nor a
/// value reference resolves to the special case; both land here.
fn cast_marker_is_not_a_value(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "`{name}` is a compile-time cast marker, not a first-class value — import it with `from typing import cast` and call the bare name (e.g. `cast(int, value)`)"
        ),
        Span::new(0, 0),
    )
}

/// The `typing.TYPE_CHECKING` marker (#790) referenced as a first-class
/// value, or called, instead of being used exactly as the (possibly
/// negated-free) test of an `if`/`elif` statement. pycc constant-folds
/// `if TYPE_CHECKING: ...` in `pycc_hir` before type-checking ever sees the
/// test expression (see `pycc_std::StdSymbolKind::TypeCheckingMarker`'s own
/// doc comment) -- this diagnostic only fires for the qualified
/// `typing.TYPE_CHECKING` spelling used somewhere else, such as `x =
/// typing.TYPE_CHECKING` or `typing.TYPE_CHECKING()`.
fn type_checking_marker_is_not_a_value(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!(
            "`{name}` is a compile-time marker, not a first-class value — use it only as the test of an `if TYPE_CHECKING:` guard"
        ),
        Span::new(0, 0),
    )
}

/// Returns `true` if `kind` is any marker symbol kind (Enum, Protocol, ABC,
/// Decorator, Annotation, Cast, or TypeChecking). Used by call-site and
/// value-reference guards to reject marker symbols used as first-class
/// values with a consistent diagnostic.
fn is_marker_kind(kind: pycc_std::StdSymbolKind) -> bool {
    matches!(
        kind,
        pycc_std::StdSymbolKind::EnumMarker
            | pycc_std::StdSymbolKind::ProtocolMarker
            | pycc_std::StdSymbolKind::AbcMarker
            | pycc_std::StdSymbolKind::DecoratorMarker
            | pycc_std::StdSymbolKind::AnnotationMarker
            | pycc_std::StdSymbolKind::CastMarker
            | pycc_std::StdSymbolKind::TypeCheckingMarker
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
        Ty::List(inner) | Ty::Set(inner) | Ty::Optional(inner) => ty_contains_param(inner),
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

/// PEP 572 (#774): walks `expr` for every `HirExpr::NamedExpr { name, .. }`
/// node at any depth and pushes each `name` into `names` (skipping one
/// already recorded, mirroring every other arm in `collect_local_names`
/// below). A walrus target binds into the *enclosing function scope*
/// (`lower_stmt`'s own placement restriction limits where a `NamedExpr` can
/// appear to an `if`/`while` test or a bare expression statement -- never
/// inside a nested function/comprehension scope), so this walk does not need
/// to worry about crossing a scope boundary the way a general free-variable
/// analysis would.
fn collect_named_expr_names_in_expr<'a>(expr: &'a HirExpr, names: &mut Vec<&'a str>) {
    match expr {
        HirExpr::NamedExpr { name, value } => {
            collect_named_expr_names_in_expr(value, names);
            if !is_local(names, name) {
                names.push(name);
            }
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => {}
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_named_expr_names_in_expr(arg, names);
            }
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            collect_named_expr_names_in_expr(left, names);
            collect_named_expr_names_in_expr(right, names);
        }
        HirExpr::UnaryOp { operand, .. } => collect_named_expr_names_in_expr(operand, names),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    collect_named_expr_names_in_expr(inner, names);
                }
            }
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                collect_named_expr_names_in_expr(e, names);
            }
        }
        HirExpr::Subscript { base, index } => {
            collect_named_expr_names_in_expr(base, names);
            collect_named_expr_names_in_expr(index, names);
        }
        HirExpr::Slice { base, start, stop, step } => {
            collect_named_expr_names_in_expr(base, names);
            for bound in [start, stop, step].into_iter().flatten() {
                collect_named_expr_names_in_expr(bound, names);
            }
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            collect_named_expr_names_in_expr(value, names);
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                collect_named_expr_names_in_expr(k, names);
                collect_named_expr_names_in_expr(v, names);
            }
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            collect_named_expr_names_in_expr(key, names);
            collect_named_expr_names_in_expr(default, names);
        }
        HirExpr::AttrGet { base, .. } => collect_named_expr_names_in_expr(base, names),
        HirExpr::MethodCall { base, args, .. } => {
            collect_named_expr_names_in_expr(base, names);
            for arg in args {
                collect_named_expr_names_in_expr(arg, names);
            }
        }
        HirExpr::GenericClassInstantiate { args, .. } => {
            for arg in args {
                collect_named_expr_names_in_expr(arg, names);
            }
        }
    }
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
            HirStmt::If { test, body, orelse } => {
                collect_named_expr_names_in_expr(test, names);
                collect_local_names(body, names);
                collect_local_names(orelse, names);
            }
            HirStmt::While { test, body } => {
                collect_named_expr_names_in_expr(test, names);
                collect_local_names(body, names);
            }
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
            HirStmt::ExprStmt(expr) => collect_named_expr_names_in_expr(expr, names),
            HirStmt::Return(_)
            | HirStmt::DictSet { .. }
            | HirStmt::AttrSet { .. }
            | HirStmt::Raise { .. } => {}
            HirStmt::Match { cases, .. } => {
                for case in cases {
                    collect_pattern_capture_names(&case.pattern, names);
                    collect_local_names(&case.body, names);
                }
            }
            // Part 3 of #382 (#542): `except*` collects local names exactly
            // like plain `try`/`except` -- its `as` binding introduces a
            // local the same way, whatever the bound type turns out to be.
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
            | HirStmt::TryStar {
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
            bind_named_expr_types_in_expr(env, local_names, value);
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
                bind_named_expr_types_in_expr(env, local_names, val);
                if let Ok(ty) = infer_expr_in(env, local_names, val) {
                    env.bind(target.clone(), ty);
                }
            } else {
                env.bind(target.clone(), annotation.clone());
            }
        }
        // PEP 572 (#774): `test` can itself contain a walrus target
        // (`if (n := f()):`), and this pass -- unlike `bind_local_types_in_body`'s
        // own recursion into `body`/`orelse` -- has no other point where
        // `test` is visited at all, so a walrus bound only in `test` was
        // never pre-bound here without this call.
        HirStmt::If { test, body, orelse } => {
            bind_named_expr_types_in_expr(env, local_names, test);
            bind_local_types_in_body(env, local_names, body);
            bind_local_types_in_body(env, local_names, orelse);
        }
        HirStmt::While { test, body } => {
            bind_named_expr_types_in_expr(env, local_names, test);
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
        // PEP 572 (#774): a bare expression statement is the other
        // placement `violates_walrus_placement` permits a walrus in
        // (`n := f()` on its own line) -- without a dedicated arm this fell
        // into the catch-all below and its walrus target was never
        // pre-bound.
        HirStmt::ExprStmt(expr) => bind_named_expr_types_in_expr(env, local_names, expr),
        _ => {}
    }
}

/// Best-effort counterpart to a dedicated walk: reuses the already-exhaustive
/// `collect_named_expr_bindings` (below) to find and bind every
/// `HirExpr::NamedExpr` reachable from `expr` at any depth, exactly as
/// `bind_local_types_in_stmt`'s `Assign`/`AnnAssign` arms already bind their
/// own targets. Its `Result` is discarded rather than propagated, matching
/// every other binding attempt in this pass -- this walk only grows `env`
/// for later resolution, it never validates, and `collect_named_expr_bindings`
/// itself still binds every target it reaches before returning any error
/// for a *later* sibling, so a discarded `Err` does not lose an earlier
/// successful binding.
fn bind_named_expr_types_in_expr(env: &mut Environment, local_names: &[&str], expr: &HirExpr) {
    let _ = collect_named_expr_bindings(env, local_names, expr);
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
    // `T | None` (PEP 604, D-197, #763, Part 1 of #747): both a bare `inner`
    // value and a bare `None` widen to `Ty::Optional(inner)` -- matching
    // D-086's "no implicit widening OR narrowing" stance means this is the
    // *only* direction: `Ty::Optional(inner)` is deliberately NOT assignable
    // back to a bare `inner` here. No flow-sensitive narrowing exists yet
    // anywhere in this crate (`is None`/`is not None` only ever produces a
    // `Ty::Bool` presence result, see `expr.rs`'s `Compare` arm) -- an
    // `Optional[int]` stays `Optional[int]` on every path, including inside
    // an `if x is not None:` branch. Narrowing is tracked as Part 2 follow-up
    // work (issue #747), deliberately deferred out of this PR: the
    // `Optional[int]` representation and `is`/`is not` presence test are
    // independently useful and verifiable (see the conformance fixture at
    // `tests/fixtures/pep_0604_union.py`, which reads the presence result
    // directly rather than an unwrapped payload) without it.
    || matches!(&to, Ty::Optional(inner) if from == Ty::None || is_assignable(from.clone(), (**inner).clone()))
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
    // Issue #769 (Part 2 of #747): any assignment to `target` kills its
    // narrowing overlay entry from this point forward -- the overlay
    // records a fact about the value `target` held at narrowing time
    // (`x is not None`), which a reassignment (even to a value that
    // happens to be compatible with the narrowed type) invalidates.
    // Unconditional and target-only: this fires for every assignment path
    // (`Assign`, `AnnAssign`, a `for` loop's own target, ...) since they all
    // route through this single function, and never touches any other
    // name's overlay entry.
    env.narrowed.remove(target);
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

/// PEP 572 (#774): walks `expr` for every `HirExpr::NamedExpr { name, value }`
/// node (an `if`/`while` test or a bare expression statement -- the only
/// placements `pycc_hir::stmt::lower_stmt`'s own placement restriction
/// allows a `NamedExpr` to survive lowering in) and binds each `name` into
/// `env` via [`check_assignment`], exactly as `HirStmt::Assign`'s own arm
/// does for an ordinary `target = value` assignment.
///
/// Walked in the expression's own left-to-right evaluation order, and each
/// binding is applied immediately (not batched) -- so a later sibling
/// sub-expression that reads an earlier walrus-bound name (e.g. `(a := 1) +
/// (b := a + 1)`) resolves correctly. `value`'s type is re-derived here via
/// `infer_expr_in` rather than threaded through from the walk that already
/// validated it (this statement's own `infer_expr`/`infer_expr_in` call,
/// made by the caller just before this one runs) -- `value` has no side
/// effects of its own beyond further nested walrus bindings, which this
/// same recursive walk also applies, so recomputing its type is
/// behavior-preserving, just not the cheapest possible implementation.
fn collect_named_expr_bindings(
    env: &mut Environment,
    local_names: &[&str],
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    match expr {
        HirExpr::NamedExpr { name, value } => {
            collect_named_expr_bindings(env, local_names, value)?;
            let ty = infer_expr_in(env, local_names, value)?;
            check_assignment(env, name, ty)
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => Ok(()),
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_named_expr_bindings(env, local_names, arg)?;
            }
            Ok(())
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            collect_named_expr_bindings(env, local_names, left)?;
            collect_named_expr_bindings(env, local_names, right)
        }
        HirExpr::UnaryOp { operand, .. } => collect_named_expr_bindings(env, local_names, operand),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    collect_named_expr_bindings(env, local_names, inner)?;
                }
            }
            Ok(())
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                collect_named_expr_bindings(env, local_names, e)?;
            }
            Ok(())
        }
        HirExpr::Subscript { base, index } => {
            collect_named_expr_bindings(env, local_names, base)?;
            collect_named_expr_bindings(env, local_names, index)
        }
        HirExpr::Slice { base, start, stop, step } => {
            collect_named_expr_bindings(env, local_names, base)?;
            for bound in [start, stop, step].into_iter().flatten() {
                collect_named_expr_bindings(env, local_names, bound)?;
            }
            Ok(())
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            collect_named_expr_bindings(env, local_names, value)
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                collect_named_expr_bindings(env, local_names, k)?;
                collect_named_expr_bindings(env, local_names, v)?;
            }
            Ok(())
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            collect_named_expr_bindings(env, local_names, key)?;
            collect_named_expr_bindings(env, local_names, default)
        }
        HirExpr::AttrGet { base, .. } => collect_named_expr_bindings(env, local_names, base),
        HirExpr::MethodCall { base, args, .. } => {
            collect_named_expr_bindings(env, local_names, base)?;
            for arg in args {
                collect_named_expr_bindings(env, local_names, arg)?;
            }
            Ok(())
        }
        HirExpr::GenericClassInstantiate { args, .. } => {
            for arg in args {
                collect_named_expr_bindings(env, local_names, arg)?;
            }
            Ok(())
        }
    }
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
    // Blocker fix (D-068 review of #780): reconcile the `narrowed` overlay
    // the same way `bindings` is reconciled just above, instead of leaving
    // it as whatever it was before this `if` ran. See
    // `narrow::join_narrowed`'s doc comment for the full soundness
    // rationale -- a name stays narrowed only if both branches still narrow
    // it to the exact same type; a kill (or a narrowing established in only
    // one branch) drops out.
    env.narrowed = narrow::join_narrowed(&body_env.narrowed, &[&orelse_env.narrowed]);
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
    // Blocker fix (D-068 review of #780): a loop may run zero or more
    // times, so a name stays narrowed after the loop only if the body
    // still narrows it to the same type it had going in -- the loop
    // running zero times is exactly "env's own narrowed map", and the loop
    // body having run is "body_env's narrowed map after the body executed
    // once", so intersecting the two covers both cases. A kill inside the
    // body (e.g. `while flag: x = None`) drops `x` out, matching
    // `join_if_branches`'s identical fix. See `narrow::join_narrowed`.
    env.narrowed = narrow::join_narrowed(&env.narrowed, &[&body_env.narrowed]);
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
    // Blocker fix (D-068 review of #780): reconcile `narrowed` the same
    // conservative way as `join_if_branches`/`join_loop_body`. `env` itself
    // (pre-match) stands in for the implicit "no case matched" path -- safe
    // to include unconditionally (exhaustive or not): it never *adds* a
    // name to the intersection, since it can only narrow the result set
    // further, so an exhaustive match that never actually needed the
    // implicit path is unaffected wherever every case agrees anyway.
    let case_narrowed_maps: Vec<&HashMap<String, Ty>> =
        case_envs.iter().map(|ce| &ce.narrowed).collect();
    env.narrowed = narrow::join_narrowed(&env.narrowed, &case_narrowed_maps);
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
        // D-068 re-review of #780 (third round, warning finding): route
        // each case body through `narrow::check_stmt_sequence[_in_function]`
        // instead of a raw per-statement loop, so a nested early-return
        // guard inside a `match` case narrows the rest of that same case
        // body -- the identical fast-path-bypass defect the `if`/`while`
        // fast-path helpers already had fixed for finding 2, but which
        // `check_match`'s own always-raw loop had never been routed
        // through in the first place.
        match return_ty {
            Some(rt) => narrow::check_stmt_sequence_in_function(
                &mut case_env,
                local_names,
                &case.body,
                rt.clone(),
            )?,
            None => narrow::check_stmt_sequence(&mut case_env, &case.body)?,
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

/// Issue #118 Part 1: fast-path helper for module-scope `if` statements
/// where neither branch introduces new bindings. Checks both branches
/// in-place without cloning env, matching the pre-#118 behavior.
fn check_if_branches_in_place(
    env: &mut Environment,
    body: &[HirStmt],
    orelse: &[HirStmt],
) -> Result<(), Diagnostic> {
    // Warning fix (D-068 review of #780): route through the
    // narrowing-aware sequence checker, not a raw per-statement loop.
    // `introduces_bindings` gates this fast path on "no new bindings", but
    // says nothing about whether a *nested* statement recognizes an
    // early-return narrowing guard (`narrow::apply_post_if_narrowing`) that
    // needs to propagate to later statements in this same body/orelse --
    // skipping that propagation here silently rejected an otherwise valid
    // nested guard shape. See `crates/pycc_types/src/narrow.rs`.
    narrow::check_stmt_sequence(env, body)?;
    narrow::check_stmt_sequence(env, orelse)
}

/// Issue #118 Part 1: fast-path helper for module-scope `while` loops
/// where the body introduces no new bindings. Checks the body in-place
/// without cloning env.
fn check_while_body_in_place(env: &mut Environment, body: &[HirStmt]) -> Result<(), Diagnostic> {
    // Warning fix (D-068 review of #780): see `check_if_branches_in_place`'s
    // identical comment.
    narrow::check_stmt_sequence(env, body)
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
        HirStmt::ExprStmt(expr) => {
            // PEP 572 (#774): bind any walrus target the expression
            // introduces (`x := value`) *before* the full `infer_expr`
            // validation pass below -- a walrus value can itself reference
            // an earlier walrus bound within the very same expression
            // (`(a := 1) + (b := a + 1)`), and `infer_expr`'s own read-only
            // walk never binds anything, so `b`'s value would otherwise see
            // `a` as never bound. `collect_named_expr_bindings` performs its
            // own `infer_expr_in` call on each walrus's `value` as it binds
            // it (in the expression's true left-to-right evaluation order),
            // so by the time the full-expression `infer_expr` call below
            // runs, every walrus name it may reference is already bound;
            // `infer_expr` still does the real work of enforcing T0050 on
            // each `NamedExpr` node and type-checking the expression as a
            // whole (a nested walrus's own `check_assignment` inside this
            // first pass can also fail, e.g. T0045/T0023, which aborts
            // before `infer_expr` ever runs -- fine, since either error
            // aborts the same statement).
            collect_named_expr_bindings(env, &[], expr)?;
            infer_expr(env, expr).map(|_| ())
        }
        HirStmt::If { test, body, orelse } => {
            // PEP 572 (#774): bind before validating, mirroring the
            // `ExprStmt` arm's own ordering rationale above -- the test
            // always executes, so this binding is unconditional relative to
            // the branch join below.
            collect_named_expr_bindings(env, &[], test)?;
            infer_expr(env, test)?; // any type is accepted as truthy for v0.1 -- Python's own truthiness has no static type restriction
            // Issue #118 Part 1: check each branch in an independent clone of
            // env, then join the results. A no-else `if` makes all body-only
            // bindings `Maybe` (the orelse clone is empty, so every body
            // binding is "one branch only" -> Maybe).
            // Issue #769 (Part 2 of #747): a narrowing-eligible test needs
            // per-branch overlay state (`narrow::apply_branch_narrowing`),
            // which only exists on a branch-local `env` clone -- force the
            // slow/cloning path even when neither branch introduces a
            // binding, so the fast path below never silently skips
            // narrowing.
            let narrowing = narrow::narrowing_target(env, test);
            // Fast path: if neither branch introduces any new bindings and
            // no narrowing applies, skip the clone+join and check both
            // branches in-place (matching the pre-#118 behavior for
            // guard-only ifs).
            if narrowing.is_none() && !introduces_bindings(body) && !introduces_bindings(orelse) {
                check_if_branches_in_place(env, body, orelse)
            } else {
                let mut body_env = env.clone();
                let mut orelse_env = env.clone();
                if let Some(target) = &narrowing {
                    narrow::apply_branch_narrowing(&mut body_env, &mut orelse_env, target);
                }
                narrow::check_stmt_sequence(&mut body_env, body)?;
                narrow::check_stmt_sequence(&mut orelse_env, orelse)?;
                join_if_branches(env, &body_env, &orelse_env)
            }
        }
        HirStmt::While { test, body } => {
            // Issue #769 follow-up (D-068 re-review round 3): a `while`
            // body can be re-entered, and `test` itself re-executes on
            // every iteration too -- prescan and drop any name `body`
            // kills *before* checking `test`, so both the test and the
            // body (fast in-place path or slow clone+join path, which
            // clones `env` after this line and so inherits the pruning)
            // see it. See `narrow::apply_kill_prescan`'s doc comment.
            narrow::apply_kill_prescan(env, body);
            // PEP 572 (#774): bind before validating -- see the `ExprStmt`
            // arm's doc comment above for why this order is required.
            collect_named_expr_bindings(env, &[], test)?;
            infer_expr(env, test)?;
            // Issue #118 Part 1: the loop body may execute zero times, so
            // every body-only binding joins back as `Maybe`.
            // Fast path: if the body introduces no bindings, check in-place.
            if !introduces_bindings(body) {
                check_while_body_in_place(env, body)
            } else {
                let mut body_env = env.clone();
                narrow::check_stmt_sequence(&mut body_env, body)?;
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
            // Issue #769 follow-up (D-068 re-review round 3): the loop
            // body can re-run, so prescan-drop any name it kills before
            // checking it. See `narrow::apply_kill_prescan`.
            narrow::apply_kill_prescan(&mut body_env, body);
            narrow::check_stmt_sequence(&mut body_env, body)?;
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
            // Issue #769 follow-up (D-068 re-review round 3): see
            // `narrow::apply_kill_prescan`.
            narrow::apply_kill_prescan(&mut body_env, body);
            narrow::check_stmt_sequence(&mut body_env, body)?;
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
        HirStmt::TryStar {
            body,
            handlers,
            orelse,
            finalbody,
        } => check_try_star_stmt(env, &[], body, handlers, orelse, finalbody, None),
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
        narrow::check_stmt_sequence_in_function(&mut env, local_names, body, resolved_return.clone())?;
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
            // `except*` shares `Try`'s termination shape exactly: a terminal
            // `finally` replaces every earlier outcome, and otherwise the
            // normal path (body or `else`) and every matched subgroup's
            // handler must all terminate.
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
            | HirStmt::TryStar {
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

/// Issue #118 Part 1: fast-path helper for function-scope `if` statements
/// where neither branch introduces new bindings.
fn check_if_branches_in_place_in_function(
    env: &mut Environment,
    local_names: &[&str],
    body: &[HirStmt],
    orelse: &[HirStmt],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    // Warning fix (D-068 review of #780): see the module-scope
    // `check_if_branches_in_place`'s identical comment -- route through the
    // narrowing-aware sequence checker so a nested early-return guard's
    // narrowing propagates to later statements even on this fast path.
    narrow::check_stmt_sequence_in_function(env, local_names, body, return_ty.clone())?;
    narrow::check_stmt_sequence_in_function(env, local_names, orelse, return_ty)
}

/// Issue #118 Part 1: fast-path helper for function-scope `while` loops
/// where the body introduces no new bindings.
fn check_while_body_in_place_in_function(
    env: &mut Environment,
    local_names: &[&str],
    body: &[HirStmt],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    // Warning fix (D-068 review of #780): see
    // `check_if_branches_in_place_in_function`'s identical comment.
    narrow::check_stmt_sequence_in_function(env, local_names, body, return_ty)
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
            // PEP 572 (#774): bind before validating -- see the module-scope
            // `check_stmt`'s `ExprStmt` arm's doc comment for why a walrus
            // value that references an earlier walrus in the same
            // expression (`(a := 1) + (b := a + 1)`) requires the binding
            // pass to run before the full-expression `infer_expr_in` check.
            // `collect_local_names` (Step 1) already added the name to
            // `local_names`. The test always executes, so this binding is
            // unconditional relative to the branch join below.
            collect_named_expr_bindings(env, local_names, test)?;
            infer_expr_in(env, local_names, test)?;
            // Issue #118 Part 1: check each branch in an independent clone of
            // env, then join the results. A no-else `if` makes all body-only
            // bindings `Maybe`.
            // Issue #769 (Part 2 of #747): see the module-scope `If` arm's
            // identical comment -- a narrowing-eligible test forces the
            // slow/cloning path.
            let narrowing = narrow::narrowing_target(env, test);
            // Fast path: if neither branch introduces any new bindings and
            // no narrowing applies, skip the clone+join and check both
            // branches in-place.
            if narrowing.is_none() && !introduces_bindings(body) && !introduces_bindings(orelse) {
                check_if_branches_in_place_in_function(
                    env,
                    local_names,
                    body,
                    orelse,
                    return_ty.clone(),
                )
            } else {
                let mut body_env = env.clone();
                let mut orelse_env = env.clone();
                if let Some(target) = &narrowing {
                    narrow::apply_branch_narrowing(&mut body_env, &mut orelse_env, target);
                }
                narrow::check_stmt_sequence_in_function(
                    &mut body_env,
                    local_names,
                    body,
                    return_ty.clone(),
                )?;
                narrow::check_stmt_sequence_in_function(
                    &mut orelse_env,
                    local_names,
                    orelse,
                    return_ty.clone(),
                )?;
                join_if_branches(env, &body_env, &orelse_env)
            }
        }
        HirStmt::While { test, body } => {
            // Issue #769 follow-up (D-068 re-review round 3): see the
            // module-scope `While` arm's identical comment and
            // `narrow::apply_kill_prescan`'s doc comment.
            narrow::apply_kill_prescan(env, body);
            // PEP 572 (#774): bind before validating, mirroring the `If`
            // arm just above.
            collect_named_expr_bindings(env, local_names, test)?;
            infer_expr_in(env, local_names, test)?;
            // Issue #118 Part 1: the loop body may execute zero times, so
            // every body-only binding joins back as `Maybe`.
            // Fast path: if the body introduces no bindings, check in-place.
            if !introduces_bindings(body) {
                check_while_body_in_place_in_function(env, local_names, body, return_ty.clone())
            } else {
                let mut body_env = env.clone();
                narrow::check_stmt_sequence_in_function(
                    &mut body_env,
                    local_names,
                    body,
                    return_ty.clone(),
                )?;
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
            // Issue #769 follow-up (D-068 re-review round 3): see
            // `narrow::apply_kill_prescan`.
            narrow::apply_kill_prescan(&mut body_env, body);
            narrow::check_stmt_sequence_in_function(
                &mut body_env,
                local_names,
                body,
                return_ty.clone(),
            )?;
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
            // Issue #769 follow-up (D-068 re-review round 3): see
            // `narrow::apply_kill_prescan`.
            narrow::apply_kill_prescan(&mut body_env, body);
            narrow::check_stmt_sequence_in_function(
                &mut body_env,
                local_names,
                body,
                return_ty.clone(),
            )?;
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
        HirStmt::ExprStmt(expr) => {
            // PEP 572 (#774): bind before validating, mirroring the
            // module-scope `check_stmt`'s `ExprStmt` arm's doc comment.
            collect_named_expr_bindings(env, local_names, expr)?;
            infer_expr_in(env, local_names, expr).map(|_| ())
        }
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
        HirStmt::TryStar {
            body,
            handlers,
            orelse,
            finalbody,
        } => check_try_star_stmt(
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
        Ty::List(elem) | Ty::Set(elem) | Ty::Optional(elem) => {
            scan_signature_ty_for_param(elem, false, found)
        }
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
        }
        | HirStmt::TryStar {
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
        // PEP 572 (#774): `target := value` — recurse into `value` only,
        // mirroring `AttrGet`'s own single-sub-expression shape just above.
        HirExpr::NamedExpr { name: _, value } => {
            reject_generic_calls_in_expr(module_env, own_name, value)
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => Ok(()),
    }
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

/// #676 (D-210): rejects a cross-MRO attribute redeclaration whose declared
/// type differs from another class's own declaration of the same attribute
/// name, anywhere in one class's own linearized MRO.
///
/// Two symptoms motivate this, both stemming from the same root cause: a
/// base-class method resolves an attribute's declared slot type from its
/// *own* MRO-attribute entry (`pycc_mir`'s `mro_attrs`/`class_def_of`), but
/// a derived class can redeclare that same attribute with an incompatible
/// type. pycc has no per-instance runtime type tag and does not
/// monomorphize a method per subclass (static dispatch, one compiled body
/// per method, `docs/TYPE_SYSTEM.md`), so the one compiled call site cannot
/// distinguish, at run time, which declaration applies -- there is no sound
/// way to coerce at the assignment site. Left unrejected, this produces an
/// undiagnosed mis-decoded read (D-141's tagged representation reads back
/// the wrong Python value) or an outright runtime abort/segfault, entirely
/// silently at `pycc check` time. See D-210 for the full rationale,
/// including why "coerce" is unsound in general (a bare, non-derived
/// instance's own correctly-typed slot would be corrupted by any
/// unconditional coercion inserted at the shared compiled call site).
///
/// The comparison is symmetric and direction-independent: for each class
/// `C`, walk `C`'s own C3-linearized MRO (`HirClassDef::mro`, which
/// includes `C` itself and every ancestor) and compare every attribute
/// name's declaring classes' own (non-inherited) `attrs` entries
/// pairwise. This also catches a diamond conflict between two sibling
/// base classes that neither is the other's ancestor, through their
/// common descendant's own MRO, even when that descendant never
/// redeclares the attribute itself.
///
/// Only a *differing* declared `Ty` triggers `T0052` -- an identical
/// redeclaration across the MRO (the existing `mro_attrs` dedup case) and
/// a same-class attribute assigned a *value* of a different (but
/// admissible, e.g. `bool` into `int`) type at a single declaration site
/// are both unaffected: this check only ever compares two distinct
/// classes' own declared attribute types, never a single class's
/// attribute against a mere assignment.
///
/// This does not reach a `@dataclass` hierarchy's own field-name
/// conflicts: `pycc_hir::class`'s dataclass lowering (see its own
/// `merged_fields`/`field_name_present` construction) already merges a
/// dataclass's inherited fields with its own by name -- keeping
/// whichever declaration is encountered first while walking the MRO
/// least-derived-first, silently discarding a differing type on a later
/// (more-derived) redeclaration of the same field name -- before this
/// check ever runs. That HIR-lowering-time merge, not `T0052`, is
/// therefore the only mechanism that resolves a dataclass field-name
/// conflict; a `@dataclass` class's own `HirClassDef::attrs` never
/// contains two entries for the same name by the time `check` sees it,
/// so this function's own MRO walk cannot observe a divergent pair for
/// it. A conflict between an ordinary (non-dataclass) class and any
/// other class in its MRO is unaffected by this and is still caught
/// here.
///
/// Called from `check`, mirroring `check_incompatible_redefinitions`'s
/// early-return-on-first-conflict shape and call-site timing (before any
/// `Environment`/signature-inference work), since it only needs each
/// class's own already-lowered `attrs` and `mro`, both populated at
/// HIR-lowering time (`pycc_hir::class`) before `pycc_types::check` ever
/// runs.
fn check_incompatible_attribute_redeclarations(hir: &HirModule) -> Result<(), Diagnostic> {
    let classes: HashMap<&str, &HirClassDef> = hir
        .class_defs
        .iter()
        .map(|(name, def)| (name.as_str(), def))
        .collect();

    for (_, class_def) in &hir.class_defs {
        // First-seen order within this class's own MRO: (attr_name,
        // owner_class_name, declared_ty). A HashMap would make which
        // conflicting pair gets reported (when several attribute names
        // conflict) depend on hash-iteration order; a Vec keeps the
        // reported pair deterministic and tied to MRO order.
        let mut declared: Vec<(&str, &str, &Ty)> = Vec::new();
        for mro_name in &class_def.mro {
            // `.expect()`, not a hand-rolled `if let Some ... else {
            // continue }`, per this crate's own established
            // coverage-gate convention for a provably-unreachable shape
            // (`pycc_hir::class`'s own `.expect(...)` precedent at its
            // `collect_init_attrs`/dataclass-field-merge MRO lookups):
            // `class_def.mro` is C3-linearized from `hir.class_defs`
            // itself, so every name in it already has a registered
            // class def by construction; a hand-rolled skip branch here
            // could never be reached by real compiler input and would
            // otherwise demand a synthetic-only test purely to satisfy
            // D-014's 100%-region gate.
            let mro_def = classes
                .get(mro_name.as_str())
                .expect("every name in a well-formed MRO has a registered class def");
            for (attr_name, ty) in &mro_def.attrs {
                if let Some(&(_, prev_owner, prev_ty)) = declared
                    .iter()
                    .find(|(name, _, _)| *name == attr_name.as_str())
                {
                    if prev_ty != ty {
                        return Err(Diagnostic::error(
                            "T0052",
                            format!(
                                "attribute `{attr_name}` is declared as `{}` in class \
                                 `{prev_owner}` and as `{}` in class `{mro_name}`, both in \
                                 the method resolution order of class `{}`",
                                prev_ty.name(),
                                ty.name(),
                                class_def.name,
                            ),
                            Span::new(0, 0),
                        ));
                    }
                } else {
                    declared.push((attr_name.as_str(), mro_name.as_str(), ty));
                }
            }
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
            HirItem::TopLevelStmt(stmt) => {
                check_stmt(&mut env, stmt)?;
                // Issue #769 (Part 2 of #747): applied uniformly with every
                // other sequential-statement-list call site in this crate
                // for consistency, even though `HirStmt::Return` (the only
                // terminator `definitely_terminates` recognizes) cannot
                // syntactically appear at module top level -- so this is a
                // structural no-op here today, not dead functionality.
                narrow::apply_post_if_narrowing(&mut env, stmt);
            }
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
    // #676 (D-210): reject a cross-MRO attribute redeclaration with a
    // differing declared type before any expression using that attribute
    // is type-checked -- see the function's own doc comment for why this
    // must be a class-definition-time rejection rather than a coercion.
    check_incompatible_attribute_redeclarations(hir)?;
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
