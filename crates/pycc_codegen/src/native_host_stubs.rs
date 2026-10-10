//! A fully native module references no CPython host helper (#1508).
//!
//! A module compiled without `CompileOptions::ext` -- neither an `--ext`
//! artifact nor an embedded executable -- links no `pycc_ext` shim, so a
//! `pycc_ext_*` declaration left in it is an undefined symbol the link
//! fails on. `object`-typed code still reaches such a module: a legacy
//! `T = TypeVar("T")` annotation lowers to the opaque `object`
//! (`docs/TYPE_SYSTEM.md`, "Generics"), so the body of a function or
//! method over `T` reads, compares, prints, returns and releases `object`
//! values through the shim's helpers.
//!
//! None of that code can run in a native build. An `object` value enters
//! compiled code only from the host (a foreign import, which makes the
//! build embedded, or an `--ext` caller), by boxing a native value, or as a
//! list display built as a CPython `list` (`HirExpr::ObjectList`); every
//! other `object` operation consumes an existing value. The type check of a
//! fully native build refuses every boxing seam and every such display with
//! `I0406` (`pycc_types::check_without_host`). So [`define_as_traps`] turns
//! every `pycc_ext_*` declaration the module still holds into an internal
//! definition whose body traps: the module then references no host symbol
//! and links, and a trap that ever ran would be a front-end defect (an
//! `object` value the check failed to refuse), never a silent wrong answer.
//!
//! This is one pass over the finished module rather than a gate at each
//! emission site, so a helper a later change starts emitting is covered
//! without anyone remembering this file. The per-site gates that already
//! exist (`object_frame::enabled`, `object_attr`, `object_release`) stay:
//! they keep the reference traffic out of a native module's live code
//! paths, which this pass does not need to reason about.

use inkwell::context::Context;
use inkwell::module::{Linkage, Module};

/// The symbol prefix every CPython host helper of the `pycc_ext` shim
/// carries (`src/ext/pycc_ext_module.c`).
const HOST_HELPER_PREFIX: &str = "pycc_ext_";

/// Gives every body-less `pycc_ext_*` declaration in `module` an internal
/// definition that traps (see the module doc). Called only for a module
/// compiled without `CompileOptions::ext`. Answers how many it defined.
pub(super) fn define_as_traps<'ctx>(context: &'ctx Context, module: &Module<'ctx>) -> usize {
    let declarations: Vec<_> = module
        .get_functions()
        .filter(|function| {
            function.count_basic_blocks() == 0
                && function
                    .get_name()
                    .to_str()
                    .is_ok_and(|name| name.starts_with(HOST_HELPER_PREFIX))
        })
        .collect();
    if declarations.is_empty() {
        return 0;
    }
    let trap = module.get_function("llvm.trap").unwrap_or_else(|| {
        module.add_function("llvm.trap", context.void_type().fn_type(&[], false), None)
    });
    let builder = context.create_builder();
    for function in &declarations {
        function.set_linkage(Linkage::Internal);
        let entry = context.append_basic_block(*function, "native_no_host");
        builder.position_at_end(entry);
        builder
            .build_call(trap, &[], "")
            .expect("build_call should not fail for llvm.trap");
        builder
            .build_unreachable()
            .expect("build_unreachable should not fail after llvm.trap");
    }
    declarations.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_host_helper_declarations_become_internal_traps() {
        let context = Context::create();
        let module = context.create_module("m");
        let ptr = context.ptr_type(inkwell::AddressSpace::default());
        let release = module.add_function(
            "pycc_ext_obj_release",
            context.void_type().fn_type(&[ptr.into()], false),
            None,
        );
        let pack = module.add_function(
            "pycc_ext_obj_pack_int",
            ptr.fn_type(&[context.i64_type().into()], false),
            None,
        );
        let runtime = module.add_function(
            "pycc_rt_int_add",
            context.i64_type().fn_type(&[], false),
            None,
        );
        assert_eq!(define_as_traps(&context, &module), 2);
        for function in [release, pack] {
            assert_eq!(function.count_basic_blocks(), 1);
            assert_eq!(function.get_linkage(), Linkage::Internal);
        }
        assert_eq!(runtime.count_basic_blocks(), 0, "a runtime import stays");
        crate::verify_module(&module);
        let ir = crate::llvm_string_to_owned(module.print_to_string());
        assert!(ir.contains("call void @llvm.trap()"), "{ir}");
        // A second pass finds nothing left to define.
        assert_eq!(define_as_traps(&context, &module), 0);
    }

    #[test]
    fn a_module_without_host_helpers_is_untouched() {
        let context = Context::create();
        let module = context.create_module("m");
        module.add_function("main", context.i64_type().fn_type(&[], false), None);
        assert_eq!(define_as_traps(&context, &module), 0);
        assert!(module.get_function("llvm.trap").is_none());
    }
}
