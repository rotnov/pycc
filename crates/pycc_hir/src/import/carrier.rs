//! The module rule that keeps a buffer-carrier from-import's spelling an
//! annotation-only name (#1380, Part 2 of #1138, D-244).
//!
//! `from numpy import ndarray` and `from numpy.typing import NDArray` run in
//! the host but bind a hidden local name (`spelling::carrier_local_name`),
//! so the spelling keeps the buffer-carrier meaning `func::annotation_to_ty`
//! gives it. In CPython the same name is the imported object everywhere, so
//! any use other than an annotation would mean something different in pycc:
//! a call `ndarray(4)` would reach pycc's own producer arm, a read
//! `x = NDArray` would fail on a name pycc does not see, and a module's own
//! `class ndarray` would win over the import. [`reject_carrier_misuse`]
//! refuses each such use, over the whole module and independently of where
//! the import sits, because lowering sees the import table only as it stands
//! at each item.

use super::spelling;
use super::type_alias::legacy_type_alias_parts;
use crate::{ImportBinding, unsupported};
use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{
    Alias, ExceptHandler, Expr, ExprContext, Identifier, Parameters, Pattern, Stmt, TypeParam,
};
use pycc_diag::Diagnostic;

/// Refuses every read of a carrier spelling outside an annotation position,
/// and every binding of it other than the carrier import itself, in a
/// module whose `imports` hold a carrier binding (#1380). One `C0001` per
/// offending site, at the site's own span, sorted by span.
///
/// The annotation positions are the parameter and return annotations, an
/// annotated assignment's annotation, the value of a `type X = ...`
/// statement, and the value of a module top-level `X: TypeAlias = ...`
/// (D-135), the only place pycc reads that value as a type; nested inside
/// an annotation (`list[NDArray]`) is still inside it. A quoted annotation
/// was already unquoted by the parser (#889), so it is a name here too.
///
/// The import itself is recognized by its shape wherever it appears, so a
/// repeated identical import and one inside an `if TYPE_CHECKING:` block are
/// both exempt. The walk does descend into a `TYPE_CHECKING` body the
/// lowering folds away; a use there is refused too, which only fails
/// closed.
///
/// A module with no carrier binding (every module but the few that write
/// the import) returns before walking anything.
pub(crate) fn reject_carrier_misuse(body: &[Stmt], imports: &[ImportBinding]) -> Vec<Diagnostic> {
    let carriers: Vec<(&str, &str)> = imports
        .iter()
        .filter_map(|binding| match binding {
            ImportBinding::Foreign {
                local_name,
                module_path,
                ..
            } => spelling::carrier_spelling(local_name)
                .map(|spelling| (spelling, module_path.as_str())),
            _ => None,
        })
        .collect();
    if carriers.is_empty() {
        return Vec::new();
    }
    let mut scan = CarrierScan {
        carriers,
        annotation_depth: 0,
        diagnostics: Vec::new(),
    };
    for stmt in body {
        match legacy_type_alias_parts(stmt) {
            Some((target, value)) => {
                scan.check_binding(&target.id, target.range);
                scan.visit_annotation(value);
            }
            None => scan.visit_stmt(stmt),
        }
    }
    let mut diagnostics = scan.diagnostics;
    diagnostics.sort_by_key(|diagnostic| diagnostic.span.map(|span| (span.start, span.end)));
    diagnostics
}

/// The [`Visitor`] behind [`reject_carrier_misuse`].
struct CarrierScan<'i> {
    /// Each imported carrier spelling with the module it came from.
    carriers: Vec<(&'i str, &'i str)>,
    /// How many annotation positions enclose the current node.
    annotation_depth: u32,
    diagnostics: Vec<Diagnostic>,
}

impl CarrierScan<'_> {
    /// The module of the carrier import spelled `name`, if any.
    fn carrier(&self, name: &str) -> Option<&str> {
        self.carriers
            .iter()
            .find(|(spelling, _)| *spelling == name)
            .map(|(_, module_path)| *module_path)
    }

    /// Refuses a read of `name` at `range` outside an annotation position
    /// when it spells a carrier.
    fn check_read<R>(&mut self, name: &str, range: R)
    where
        std::ops::Range<u32>: From<R>,
    {
        if self.annotation_depth > 0 {
            return;
        }
        if let Some(module_path) = self.carrier(name) {
            self.diagnostics.push(unsupported(
                format!(
                    "reading `{name}` outside a type annotation is not supported yet: \
                     `from {module_path} import {name}` keeps the name's buffer-annotation \
                     meaning in pycc (D-244), so it may only annotate"
                ),
                range,
            ));
        }
    }

    /// Refuses a binding of `name` at `range` when it spells a carrier.
    fn check_binding<R>(&mut self, name: &str, range: R)
    where
        std::ops::Range<u32>: From<R>,
    {
        if let Some(module_path) = self.carrier(name) {
            self.diagnostics.push(unsupported(
                format!(
                    "binding `{name}` in a module that imports it with `from {module_path} \
                     import {name}` is not supported yet: the import keeps the name's \
                     buffer-annotation meaning in pycc (D-244)"
                ),
                range,
            ));
        }
    }

    fn check_identifier(&mut self, identifier: &Identifier) {
        self.check_binding(identifier.as_str(), identifier.range);
    }

    /// Checks the name one import alias binds. `from_module` is the
    /// `(module, level)` of a `from` import, whose unaliased carrier pair is
    /// the import this rule protects rather than a second binding.
    fn check_alias(&mut self, alias: &Alias, from_module: Option<(&str, u32)>) {
        let name = alias.name.as_str();
        match &alias.asname {
            Some(asname) => self.check_identifier(asname),
            None if from_module.is_some_and(|(module, level)| {
                spelling::carrier_from_import(module, name, level)
            }) => {}
            // An unaliased `import a.b` binds its first segment, `a`.
            None => self.check_binding(
                name.split_once('.').map_or(name, |(first, _)| first),
                alias.range,
            ),
        }
    }
}

impl<'a> Visitor<'a> for CarrierScan<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(def) => self.check_identifier(&def.name),
            Stmt::ClassDef(class) => self.check_identifier(&class.name),
            Stmt::Global(global) => {
                for name in &global.names {
                    self.check_identifier(name);
                }
            }
            Stmt::Nonlocal(nonlocal) => {
                for name in &nonlocal.names {
                    self.check_identifier(name);
                }
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    self.check_alias(alias, None);
                }
            }
            Stmt::ImportFrom(import) => {
                let module = import.module.as_deref().unwrap_or("");
                for alias in &import.names {
                    self.check_alias(alias, Some((module, import.level)));
                }
            }
            // `walk_stmt` visits a `type` statement's value as a plain
            // expression, but it is an annotation position.
            Stmt::TypeAlias(alias) => {
                self.visit_annotation(&alias.value);
                if let Some(type_params) = &alias.type_params {
                    self.visit_type_params(type_params);
                }
                self.visit_expr(&alias.name);
                return;
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_annotation(&mut self, expr: &'a Expr) {
        self.annotation_depth += 1;
        visitor::walk_annotation(self, expr);
        self.annotation_depth -= 1;
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr {
            match name.ctx {
                ExprContext::Load => self.check_read(&name.id, name.range),
                // `Store` and `Del` (`del ndarray`) both bind the name.
                _ => self.check_binding(&name.id, name.range),
            }
        }
        visitor::walk_expr(self, expr);
    }

    fn visit_parameters(&mut self, parameters: &'a Parameters) {
        for parameter in parameters.iter() {
            self.check_identifier(parameter.name());
        }
        visitor::walk_parameters(self, parameters);
    }

    fn visit_type_param(&mut self, type_param: &'a TypeParam) {
        self.check_identifier(type_param.name());
        visitor::walk_type_param(self, type_param);
    }

    fn visit_except_handler(&mut self, handler: &'a ExceptHandler) {
        let ExceptHandler::ExceptHandler(except) = handler;
        if let Some(name) = &except.name {
            self.check_identifier(name);
        }
        visitor::walk_except_handler(self, handler);
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        let captured = match pattern {
            Pattern::MatchAs(pattern) => pattern.name.as_ref(),
            Pattern::MatchStar(pattern) => pattern.name.as_ref(),
            Pattern::MatchMapping(pattern) => pattern.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = captured {
            self.check_identifier(name);
        }
        visitor::walk_pattern(self, pattern);
    }
}
