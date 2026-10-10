//! A native value moved into an `object` slot (Part 2 of #1387, D-258's
//! boxing amendment).
//!
//! D-258 makes `object` (and, in an `--ext` module, `Any`) the opaque
//! CPython top type. `crate::is_assignable` keeps admitting `object` only
//! from `object`: it is shared by branch joins, container element checks,
//! protocol members and generic bounds, none of which has a conversion. The
//! one conversion that exists is the shim's packers (`pycc_ext_obj_pack_*`),
//! which turn a native `int`, `float`, `bool`, `str` or a compiled
//! instance's `PyccExtInstance` carrier into the `PyObject *` CPython would
//! hold, plus `pycc_ext_obj_none` for `None`.
//!
//! [`admits`] is the one statement of which values those are. Each seam
//! that moves exactly one value into exactly one slot ORs it into its own
//! assignability test:
//!
//! - a compiled function, method, constructor, static, class or `super()`
//!   call's argument (`expr.rs`, `class::check_call_args`);
//! - an annotated binding `y: object = v` and a later plain rebinding of an
//!   `object`-typed name (`crate::check_stmt*`, `crate::check_assignment`);
//! - `return v` in a function declared `-> object` (`crate::check_stmt_in_function`
//!   and the solver's `HirStmt::Return` arm);
//! - an attribute store into an `object`-typed slot, including a property
//!   setter's `object` parameter (`class::attr_set`).
//!
//! Everything else keeps the strict rule: a native container (D-258 rule 4:
//! a copy would break aliasing), an `Optional[T]` value (its `None` and its
//! payload have no single packer), an enum member and an exception
//! instance. `pycc_mir` inserts `MirExpr::ObjectBox` at the binding,
//! rebinding and attribute-store seams, and `pycc_codegen` boxes a call
//! argument and a returned value directly (its `object_box` module).
//!
//! A foreign-class annotation is also `Ty::Object` (Part 1 of #1367), so a
//! native value is admitted into one too. Annotations are not enforced at
//! runtime, so the program behaves as CPython runs it; only the static
//! strictness of a foreign-class annotation is lost.

use crate::env::Environment;
use crate::foreign;
use pycc_diag::Diagnostic;
use pycc_hir::{HirExpr, Ty};

/// Whether a value of type `from` moved into a slot of type `to` is a
/// native value boxed into an `object` slot.
///
/// `false` when `to` is not `object`, and when `from` already is one (the
/// ordinary assignability check admits that without a conversion).
pub(crate) fn admits(env: &Environment, from: &Ty, to: &Ty) -> bool {
    matches!(to, Ty::Object)
        && !matches!(from, Ty::Object)
        && (matches!(from, Ty::None) || foreign::is_object_operand(env, from))
}

/// [`admits`] for a value whose expression is known, refusing (`I0404`)
/// a class method's own `cls`, which is typed as an instance of its class
/// but holds none ([`foreign::refuse_classmethod_cls`]).
///
/// Every boxing seam with an expression in hand calls this one: an
/// annotated binding, a plain rebinding, a `return`, an attribute store and
/// every call shape's arguments (`class::check_call_args` takes the
/// argument expressions too). Only an alias (`c = cls; f(c)`) is not
/// traced; it reaches the shim's null guard and raises `SystemError`, as it
/// already does at a foreign call.
pub(crate) fn admits_value(
    env: &Environment,
    value: &HirExpr,
    from: &Ty,
    to: &Ty,
) -> Result<bool, Diagnostic> {
    if !admits(env, from, to) {
        return Ok(false);
    }
    foreign::refuse_classmethod_cls(
        env,
        value,
        "boxing a class method's `cls` into an `object` slot",
    )?;
    if hostless() {
        return Err(boxing_without_host(&format!(
            "a native `{}` value",
            from.name()
        )));
    }
    Ok(true)
}

thread_local! {
    /// Whether the check running on this thread is a fully native build's
    /// ([`check_without_host`]).
    static HOSTLESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The stable code a fully native build refuses a boxing seam with (#1508).
pub const NATIVE_BOXING_CODE: &str = "I0406";

/// Runs `check` as the type check of a fully native build (#1508): one
/// that is neither an `--ext` artifact nor an embedded executable, and so
/// links no CPython host. Every boxing seam (`admits_value`, and the bare
/// `return`/`return None` of `crate::object_none`), and every list display
/// built as a CPython `list` (`crate::foreign::list_display`), is then
/// refused with [`NATIVE_BOXING_CODE`] instead of admitted: its helper
/// (`pycc_ext_obj_pack_*`, `pycc_ext_obj_none`,
/// `pycc_ext_obj_build_list`) lives in the host shim,
/// which such a build does not link, so no `object` value can exist in it.
///
/// The mode is a scoped thread-local rather than an `Environment` field
/// because the check builds many environments across its phases
/// (signature inference, the solver, the per-function walks,
/// monomorphization), and every one of them must see the same answer. The
/// guard restores the previous value on every exit, unwinding included.
pub fn check_without_host<R>(check: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOSTLESS.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(HOSTLESS.with(|cell| cell.replace(true)));
    check()
}

/// Whether the running check is a fully native build's
/// ([`check_without_host`]).
pub(crate) fn hostless() -> bool {
    HOSTLESS.with(std::cell::Cell::get)
}

/// The `I0406` refusal of moving `what` into an `object` slot in a fully
/// native build ([`check_without_host`]).
pub(crate) fn boxing_without_host(what: &str) -> Diagnostic {
    Diagnostic::error(
        NATIVE_BOXING_CODE,
        format!(
            "{what} cannot be boxed into a CPython `object` slot (such as a legacy `TypeVar` \
             annotation) in a native build; use a PEP 695 type parameter (`def f[T](x: T) -> T`) \
             or `pycc build --ext`"
        ),
        pycc_diag::Span::new(0, 0),
    )
    .with_help(
        "boxing needs the CPython host, which only `pycc build --ext` and an embedded executable \
         link: use a PEP 695 type parameter (`def f[T](x: T) -> T`), which pycc compiles \
         natively per call site, or build with `--ext`",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_and_none_box_into_object() {
        let env = Environment::new();
        for from in [Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::None] {
            assert!(admits(&env, &from, &Ty::Object), "{from:?}");
        }
    }

    #[test]
    fn object_containers_optional_and_unknown_instances_do_not_box() {
        let env = Environment::new();
        for from in [
            Ty::Object,
            Ty::List(Box::new(Ty::Int)),
            Ty::Optional(Box::new(Ty::Int)),
            Ty::Instance(Box::new("Missing".to_string())),
        ] {
            assert!(!admits(&env, &from, &Ty::Object), "{from:?}");
        }
    }

    fn hir(src: &str) -> pycc_hir::HirModule {
        let module = pycc_parser::parse(src).expect("test fixture must parse");
        pycc_hir::lower_checked(&module).expect("test fixture must lower")
    }

    const TYPE_VAR: &str = "from typing import TypeVar\nT = TypeVar(\"T\")\n";

    /// #1508: every boxing seam a legacy type variable reaches is admitted
    /// by the ordinary check and refused with `I0406` by a fully native
    /// build's.
    #[test]
    fn a_hostless_check_refuses_every_boxing_seam_with_i0406() {
        for (seam, body) in [
            ("argument", "def f(k: T) -> T:\n    return k\nf(3)\n"),
            ("None argument", "def f(k: T) -> None:\n    pass\nf(None)\n"),
            ("return value", "def f() -> T:\n    return 3\n"),
            ("bare return", "def f() -> T:\n    return\n"),
            // The empty-container pre-pass rewrites a list display bound to
            // an object slot into `HirExpr::ObjectList` before the check, so
            // it reaches no `admits_value` seam (`foreign::list_display`).
            ("list display", "def f() -> None:\n    x: T = [1]\n"),
            ("empty list display", "def f() -> None:\n    x: T = []\n"),
            (
                "list display in a value position",
                "def f(c: bool) -> None:\n    x: T = [1] if c else []\n",
            ),
            (
                "rebound to a list display",
                "def f(k: T) -> None:\n    s = k\n    s = []\n",
            ),
            (
                "list display stored into an attribute",
                "from typing import Generic\nclass B(Generic[T]):\n    v: T\n    def __init__(self, v: T) -> None:\n        self.v = v\n    def reset(self) -> None:\n        self.v = self.v or [1]\n",
            ),
            ("return None", "def f() -> T:\n    return None\n"),
            (
                "constructor",
                "from typing import Generic\nclass B(Generic[T]):\n    def __init__(self, v: T) -> None:\n        self.v = v\nB(\"s\")\n",
            ),
        ] {
            let hir = hir(&format!("{TYPE_VAR}{body}"));
            crate::check_and_resolve_all(&hir)
                .unwrap_or_else(|errors| panic!("{seam}: the hosted check admits it: {errors:?}"));
            let errors = check_without_host(|| crate::check_and_resolve_all(&hir)).expect_err(seam);
            assert_eq!(errors[0].code, NATIVE_BOXING_CODE, "{seam}: {errors:?}");
            assert!(errors[0].message.contains("native build"), "{seam}");
            if seam.contains("list display") {
                assert!(
                    errors[0].message.contains("list display"),
                    "{seam}: {errors:?}"
                );
            }
        }
    }

    /// A type-variable function that is never handed a native value, and a
    /// bare `return` from a `-> None` function, are not boxing seams.
    #[test]
    fn a_hostless_check_admits_code_that_boxes_nothing() {
        let hir = hir(&format!(
            "{TYPE_VAR}def f(k: T) -> T:\n    x = k\n    return x\ndef g() -> None:\n    return\ng()\nprint(1)\n"
        ));
        check_without_host(|| crate::check_and_resolve_all(&hir)).expect("nothing is boxed");
    }

    /// The mode is scoped: it is off outside the closure, nests, and is
    /// restored when the closure unwinds.
    #[test]
    fn the_hostless_mode_is_scoped_to_its_closure() {
        assert!(!hostless());
        check_without_host(|| {
            assert!(hostless());
            check_without_host(|| assert!(hostless()));
            assert!(hostless(), "an inner scope restores the outer one");
        });
        assert!(!hostless());
        let unwound = std::panic::catch_unwind(|| check_without_host(|| panic!("unwind")));
        assert!(unwound.is_err());
        assert!(!hostless(), "an unwinding check restores the mode");
    }

    #[test]
    fn only_an_object_slot_boxes() {
        let env = Environment::new();
        assert!(!admits(&env, &Ty::Int, &Ty::Float));
        assert!(!admits(&env, &Ty::Str, &Ty::Str));
    }
}
