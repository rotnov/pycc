//! Part 3 of #1387 (#1476): the shared recognizer and gate for narrowing a
//! CPython `object` back to a native type under an `isinstance` guard.
//!
//! The checker (`pycc_types::narrow`) and the MIR lowering (`pycc_mir`)
//! both call this module, for the same reason [`crate::optional_none_test`]
//! lives here: `pycc_mir` does not depend on `pycc_types`, and two
//! independent copies of the pattern would drift. Only the scope lookup --
//! whether the guarded name's declared type is `object`, and whether the
//! class name or `isinstance` itself is shadowed -- is per layer.

use crate::{HirClassDef, HirExpr, Ty, UnaryOpKind, is_builtin_exception_class};

/// Which branch of an `if` an `isinstance` guard narrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsInstancePolarity {
    /// `isinstance(name, C)`: the `if` body runs only when the test holds.
    Positive,
    /// `not isinstance(name, C)`: the `orelse`, and the continuation after
    /// an `if` whose body definitely terminates, run only when it holds.
    Negated,
}

/// Recognizes `test` as `isinstance(name, Class)` or
/// `not isinstance(name, Class)`, where both arguments are bare names, and
/// answers `(name, class name, polarity)`. Every other shape -- a tuple of
/// classes, an attribute operand, an `and`/`or` compound, a keyword or a
/// third argument -- answers `None`.
///
/// Purely syntactic: the caller decides whether `isinstance` is the builtin
/// (no user function of that name), whether `name` is an `object` slot,
/// and whether `Class` is unshadowed.
pub fn isinstance_test(test: &HirExpr) -> Option<(&str, &str, IsInstancePolarity)> {
    let (call, polarity) = match test {
        HirExpr::UnaryOp {
            op: UnaryOpKind::Not,
            operand,
        } => (operand.as_ref(), IsInstancePolarity::Negated),
        other => (other, IsInstancePolarity::Positive),
    };
    let HirExpr::Call { callee, args } = call else {
        return None;
    };
    match (callee.as_str(), args.as_slice()) {
        ("isinstance", [HirExpr::Name(name), HirExpr::Name(class)]) => {
            Some((name.as_str(), class.as_str(), polarity))
        }
        _ => None,
    }
}

/// The native type an `object` narrows to under `isinstance(o, class)`, or
/// `None` when this part does not narrow that class.
///
/// `lookup_class` resolves a compiled class by name in the caller's class
/// table. The caller has already established that `class` is not shadowed
/// by a local, parameter or global binding.
///
/// - `int`, `float`, `bool` and `str` narrow to the same scalar type.
/// - A compiled class narrows to `Ty::Instance` when
///   [`HirClassDef::admits_isinstance_narrowing`] holds.
/// - Every other class -- `list`, `dict`, `tuple`, a foreign class, an
///   unknown name -- keeps the operand `object`.
pub fn isinstance_narrow_target<'a>(
    class: &str,
    lookup_class: impl FnOnce(&str) -> Option<&'a HirClassDef>,
) -> Option<Ty> {
    match class {
        "int" => Some(Ty::Int),
        "float" => Some(Ty::Float),
        "bool" => Some(Ty::Bool),
        "str" => Some(Ty::Str),
        _ => lookup_class(class)
            .filter(|def| def.admits_isinstance_narrowing())
            .map(|def| Ty::Instance(Box::new(def.name.clone()))),
    }
}

impl HirClassDef {
    /// Whether an `object` may be narrowed to an instance of this class
    /// under an `isinstance` guard (#1476): a regular, non-generic class.
    ///
    /// Refused, keeping the operand `object`:
    /// - an enum, whose members are allocated without the layout descriptor
    ///   a carrier names its type from;
    /// - an exception class, which crosses through the D-244 exception
    ///   bridge rather than a carrier (the same test as
    ///   `pycc_types::foreign::is_carriable_instance`);
    /// - a protocol, which is structural and has no carrier;
    /// - a PEP 695 generic class (`class C[T]`), whose instance type would
    ///   need a type argument the guard does not name. A `Generic[T]` base is
    ///   erased at lowering and leaves `type_param` unset, so such a class is
    ///   admitted.
    pub fn admits_isinstance_narrowing(&self) -> bool {
        !self.is_enum
            && !self.is_protocol
            && self.type_param.is_none()
            && self.exception_type_tag.is_none()
            && !self
                .mro
                .iter()
                .any(|entry| is_builtin_exception_class(entry))
    }
}

#[cfg(test)]
mod tests;
