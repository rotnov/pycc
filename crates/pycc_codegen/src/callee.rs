//! Where a user-function call resolves its target relative to its
//! arguments (#1490).
//!
//! Issue #22: an ordinary user function dispatches indirectly through its
//! function-pointer slot, which is null until the `def` executes, and a
//! null slot raises `NameError`. CPython looks the callee up at a fixed
//! point in the call's evaluation order, so the slot check has to happen
//! at the same point for the raise to be observed in the same order:
//!
//! * `g(h())` looks `g` up before evaluating `h()`, so the slot is checked
//!   before any argument ([`CallTarget::Resolved`]).
//! * `make().m(h())` evaluates the receiver `make()` before the attribute
//!   lookup, and the arguments after it. MIR lowers a method call to a
//!   `Call` of the method's mangled name with the receiver as `args[0]`, so
//!   the slot is checked between `args[0]` and the rest
//!   ([`CallTarget::AfterReceiver`]).
//!
//! An unbound method of any kind -- regular, class, static, a property
//! setter or a receiver-exact copy -- reports its class, not its mangled
//! name ([`unbound_name`]): CPython's `C.s()` above `class C` fails looking
//! `C` up, `name 'C' is not defined`, before it ever reaches `s`.
//!
//! New code lands here rather than in the oversized `lib.rs` (AGENTS.md's
//! "Keep source files decomposable").

use crate::UserFunction;
use crate::exception;
use crate::ext_thunk;
use crate::rt_fns::RtFns;
use inkwell::context::Context;
use inkwell::values::{FunctionValue, PointerValue};

/// A user function's call target once its function-pointer slot has been
/// checked: the slot's loaded pointer, or a monomorphized specialization's
/// own function.
#[derive(Clone, Copy)]
pub(crate) enum ResolvedCallee<'ctx> {
    Indirect(PointerValue<'ctx>),
    Direct(FunctionValue<'ctx>),
}

/// When a call checks its callee's slot.
#[derive(Clone, Copy)]
pub(crate) enum CallTarget<'ctx, 'name> {
    /// Already checked, before any argument is evaluated.
    Resolved(ResolvedCallee<'ctx>),
    /// Checked once `args[0]`, the method's receiver, is evaluated and
    /// before the remaining arguments are; an unbound slot reports
    /// `unbound_name` (see [`resolve_user_callee`]).
    AfterReceiver { unbound_name: Option<&'name str> },
}

/// The name an unbound `callee_name` reports, when it is not the callee's
/// own: a method-like item (`Class.member...`, see [`receiver_leads`])
/// reports its class, which is what CPython fails to find. A module-level
/// function reports itself (`None`).
pub(crate) fn unbound_name(callee_name: &str) -> Option<&str> {
    callee_name.split_once('.').map(|(class, _)| class)
}

/// Whether a call of `callee_name` carries its receiver as `args[0]`.
///
/// Every method-like item `pycc_hir` lowers is named `Class.member...`
/// (`pycc_hir::class::method`), and every one but a `@staticmethod`
/// (`.static`) takes its receiver -- `self`, or a class method's `cls`
/// -- as the first argument; a receiver-exact copy keeps that spelling
/// (`pycc_hir::inherited_copy_name`, which copies no static method). A
/// module-level function's name is an identifier, which has no `.`.
pub(crate) fn receiver_leads(callee_name: &str) -> bool {
    callee_name.contains('.') && !callee_name.ends_with(".static")
}

/// Resolves `user_function`'s call target at the current insertion point.
///
/// The load, the null check and its `NameError` path are shared with
/// #1050's `ext` export thunks, which dispatch through the same slot: see
/// `ext_thunk::emit_fnptr_dispatch_guard`. Here the null path branches to
/// the innermost exception target, releasing the live temporaries on the
/// way exactly as a failing operation does, and the builder is left on the
/// non-null path. `unbound_name` overrides the reported name (a constructor
/// reports its class). Monomorphized generic specializations (`0gen_...`
/// names) have no `fn_ptr_global` -- they are compiler-generated, not
/// user-defined, and dispatch directly.
pub(crate) fn resolve_user_callee<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_function: &UserFunction<'ctx>,
    unbound_name: Option<&str>,
) -> ResolvedCallee<'ctx> {
    if let Some(direct_value) = user_function.direct_value {
        return ResolvedCallee::Direct(direct_value);
    }
    ResolvedCallee::Indirect(ext_thunk::emit_fnptr_dispatch_guard(
        context,
        builder,
        module,
        rt,
        user_function,
        unbound_name,
        || exception::jump_to_exception_target(context, builder, rt),
    ))
}

#[cfg(test)]
mod tests {
    use super::{receiver_leads, unbound_name};

    #[test]
    fn a_method_like_callee_reports_its_class() {
        assert_eq!(unbound_name("C.m"), Some("C"));
        assert_eq!(unbound_name("C.s.static"), Some("C"));
        assert_eq!(unbound_name("C.cm.classmethod"), Some("C"));
        assert_eq!(unbound_name("C.p.setter"), Some("C"));
        assert_eq!(unbound_name("D.m.0super_C"), Some("D"));
        assert_eq!(unbound_name("late"), None);
    }

    #[test]
    fn a_method_or_class_method_leads_with_its_receiver() {
        assert!(receiver_leads("C.m"));
        assert!(receiver_leads("C.p.setter"));
        assert!(receiver_leads("C.make.classmethod"));
        assert!(receiver_leads("D.m.0super_C"));
    }

    #[test]
    fn a_function_or_static_method_has_no_receiver_argument() {
        assert!(!receiver_leads("late"));
        assert!(!receiver_leads("C.create.static"));
    }
}
