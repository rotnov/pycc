//! PEP 562 module hooks at the `--ext` boundary (#1467).
//!
//! CPython consults a module's own `__getattr__` when an attribute read on
//! the module finds nothing in its `__dict__`, and builtin `dir(module)`
//! calls the module's own `__dir__`. Both are looked up in the module's
//! dict, so the hook is honoured exactly when the dict holds it. D-244
//! rule 1's export set borrows D-038's public-name predicate, which refuses
//! every dunder, so before #1467 a compiled `def __getattr__` never reached
//! the extension's dict and the host silently ignored it.
//!
//! An export named here is published as an ordinary `METH_FASTCALL`
//! module function, but only **after** the module body has run: the
//! wrapper calls through a `fnptr_` slot the body fills, and importlib's
//! `_init_module_attrs` reads attributes of the half-initialised module
//! before `Py_mod_exec`, so a hook installed from `PyModuleDef.m_methods`
//! would be called through a null slot. `pycc_ext_exec_module` adds the
//! `pycc_ext_module_hooks[]` table once the body has succeeded, which also
//! matches CPython, where the hook enters the dict when its `def` executes.
//!
//! Which hooks are published is read from the **entry** module's own
//! source, because the export set is collected from D-222's linked program,
//! where a helper module's `def __getattr__` is an indistinguishable
//! `HirItem::Function`. CPython never applies a helper's hook to the entry
//! module, so only a `def` at the entry module's top level counts. An
//! import that binds a hook name in the entry module would put a hook in
//! CPython's dict that this boundary cannot publish, so it is refused with
//! a located `C0001` rather than ignored.

use std::collections::BTreeSet;

use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{Alias, ModModule, Stmt};
use pycc_diag::{Diagnostic, Span};

/// The module-level names CPython calls implicitly (PEP 562).
pub(crate) const MODULE_HOOKS: [&str; 2] = ["__getattr__", "__dir__"];

/// Whether `name` is one of [`MODULE_HOOKS`].
pub(crate) fn is_module_hook(name: &str) -> bool {
    MODULE_HOOKS.contains(&name)
}

/// The hooks the entry module defines with a top-level `def`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct EntryHooks {
    defined: BTreeSet<String>,
}

impl EntryHooks {
    /// The hooks of `source`, or none when it does not parse. The frontend
    /// has already parsed the same text successfully, so the empty set is a
    /// fail-closed default rather than a reachable outcome, exactly as for
    /// [`super::SourceSignatures::from_source`].
    pub(crate) fn from_source(source: &str) -> Result<Self, Vec<Diagnostic>> {
        pycc_parser::parse(source)
            .map_or_else(|_| Ok(Self::default()), |module| Self::scan(&module))
    }

    /// Scans the entry module: a top-level `def` of a hook name defines it,
    /// and every import alias that binds a hook name -- at the top level or
    /// anywhere inside a top-level compound statement, but not inside a
    /// function or class body, whose bindings are not module attributes --
    /// is one located `C0001`. The scan is deliberately over-inclusive: it
    /// does not evaluate a guard, so an import CPython never executes --
    /// under `if TYPE_CHECKING:`, say -- is refused too. That costs a
    /// spurious refusal of a rare spelling, never a silently ignored hook.
    pub(crate) fn scan(module: &ModModule) -> Result<Self, Vec<Diagnostic>> {
        let defined = module
            .body
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::FunctionDef(def) if is_module_hook(def.name.as_str()) => {
                    Some(def.name.to_string())
                }
                _ => None,
            })
            .collect();
        let mut imports = ImportBindings::default();
        imports.visit_body(&module.body);
        if imports.refusals.is_empty() {
            Ok(Self { defined })
        } else {
            Err(imports.refusals)
        }
    }

    /// The hook names to publish.
    pub(crate) fn defined(&self) -> &BTreeSet<String> {
        &self.defined
    }
}

/// Collects a refusal for every import alias that binds a hook name at
/// module scope.
#[derive(Default)]
struct ImportBindings {
    refusals: Vec<Diagnostic>,
}

impl ImportBindings {
    fn check(&mut self, alias: &Alias, bound: &str) {
        if is_module_hook(bound) {
            let range: std::ops::Range<u32> = alias.range.into();
            self.refusals.push(import_binding_refusal(
                bound,
                Span::new(range.start, range.end),
            ));
        }
    }
}

impl<'a> Visitor<'a> for ImportBindings {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            // A local scope: nothing bound in it is a module attribute.
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => return,
            // `import a.b` binds `a`; `import a.b as c` binds `c`.
            Stmt::Import(import) => {
                for alias in &import.names {
                    let bound = alias.asname.as_ref().map_or_else(
                        || alias.name.as_str().split('.').next().unwrap_or_default(),
                        |asname| asname.as_str(),
                    );
                    self.check(alias, bound);
                }
            }
            Stmt::ImportFrom(import) => {
                for alias in &import.names {
                    self.check(alias, alias.asname.as_ref().unwrap_or(&alias.name).as_str());
                }
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }
}

/// The located `C0001` for an import that binds the hook `hook`.
fn import_binding_refusal(hook: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "--ext cannot publish a module `{hook}` bound by an import -- CPython calls a \
             module's own `{hook}` implicitly (PEP 562), and pycc publishes only a \
             `def {hook}` written at the top level of the entry module; define it there, or \
             build without --ext (#1467)"
        ),
        span,
    )
}

#[cfg(test)]
#[path = "module_hooks_tests.rs"]
mod tests;
