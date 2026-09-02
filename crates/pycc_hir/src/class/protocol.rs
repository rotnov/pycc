//! Protocol class lowering (#380, PR-20), extracted from `class.rs` per
//! AGENTS.md's file-decomposition rule (issue #890; tracking issue #548),
//! following `class/mro.rs`'s precedent. The body is the original
//! `lower_protocol_class`'s, unchanged.

use super::{ClassAnnotationInfo, HirClassDef, ProtocolMember, is_declaration_body};
use crate::{HirItem, Ty, unsupported};
use pycc_ast::{Expr, Stmt};
use pycc_diag::Diagnostic;

/// #380 (PR-20): Lowers a protocol class body. Protocol methods (with
/// `...` or `pass` bodies) are recorded as `ProtocolMember::Method` and
/// are NOT lowered to `HirItem::Function`s. Protocol annotated assignments
/// (`x: int`) are recorded as `ProtocolMember::Attribute`. Protocol
/// members from base protocols are inherited. A protocol method with an
/// implementation body is rejected with `C0001`. A protocol `__init__` is
/// rejected with `C0001`.
#[allow(clippy::too_many_arguments)]
pub(super) fn lower_protocol_class(
    def: &pycc_ast::StmtClassDef,
    class_name: String,
    bases: Vec<String>,
    mro: Vec<String>,
    type_param: Option<String>,
    runtime_checkable: bool,
    defined_classes: &[(String, HirClassDef)],
    class_name_defs: &[ClassAnnotationInfo],
) -> Result<(HirClassDef, Vec<HirItem>), Diagnostic> {
    let mut protocol_members: Vec<ProtocolMember> = Vec::new();
    // Inherit protocol members from base protocols.
    for base_name in &bases {
        // This should have been validated already in `lower_class`.
        // Using `.expect()` (whose panic path lives in libcore, outside
        // this crate's instrumented regions) instead of a
        // `let .. else { panic!() }` avoids a permanently-uncovered
        // branch under D-014's 100%-region coverage gate.
        let base_def = &defined_classes
            .iter()
            .find(|(n, _)| n == base_name)
            .expect("pycc_hir: internal error: protocol base not found in defined_classes -- lower_class should have validated this")
            .1;
        for member in &base_def.protocol_members {
            // Only add inherited members that are not redeclared in this
            // class (redeclarations are added below from the class body).
            let name = match member {
                ProtocolMember::Method { name, .. } => name,
                ProtocolMember::Attribute { name, .. } => name,
            };
            if !protocol_members.iter().any(|m| match m {
                ProtocolMember::Method { name: n, .. } => n == name,
                ProtocolMember::Attribute { name: n, .. } => n == name,
            }) {
                protocol_members.push(member.clone());
            }
        }
    }
    for stmt in &def.body {
        match stmt {
            Stmt::FunctionDef(method_def) => {
                let method_name = method_def.name.to_string();
                // Reject `__init__` in a protocol class.
                if method_name == "__init__" {
                    return Err(unsupported(
                        format!(
                            "a protocol class `{class_name}` cannot define `__init__` -- \
                             protocols are not instantiated"
                        ),
                        method_def.range,
                    ));
                }
                // Reject decorators on protocol methods.
                if !method_def.decorator_list.is_empty() {
                    return Err(unsupported(
                        format!(
                            "decorators on protocol method `{class_name}.{method_name}` are \
                             not supported yet"
                        ),
                        method_def.range,
                    ));
                }
                // Reject generic protocol methods.
                if method_def.type_params.is_some() {
                    return Err(unsupported(
                        format!(
                            "a generic protocol method `{class_name}.{method_name}` is not \
                             supported yet"
                        ),
                        method_def.range,
                    ));
                }
                // The method body must be declaration-style (`...` or
                // `pass`).
                if !is_declaration_body(&method_def.body) {
                    return Err(unsupported(
                        format!(
                            "a protocol method `{class_name}.{method_name}` must have a \
                             declaration-style body (`...` or `pass`), not an implementation"
                        ),
                        method_def.range,
                    ));
                }
                // Lower the method's parameter and return types.
                // `self` is handled specially (assigned
                // `Ty::Instance(class_name)` directly, bypassing
                // `annotation_to_ty`), matching how `lower_method`
                // handles it for regular methods. The remaining
                // parameters go through `lower_arg_list`.
                let all_args = &method_def.parameters.args;
                let (params, return_ty) =
                    if !all_args.is_empty() && all_args[0].parameter.name.as_str() == "self" {
                        // Strip `self` and lower the rest.
                        let rest = &all_args[1..];
                        let method_is_public = !method_name.starts_with('_');
                        let p = crate::lower_arg_list(
                            rest,
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        let r = crate::lower_return_annotation(
                            method_def.returns.as_deref(),
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        (p, r)
                    } else {
                        // No `self` parameter — this is unusual for a
                        // protocol method but we handle it gracefully by
                        // lowering all parameters.
                        let method_is_public = !method_name.starts_with('_');
                        let p = crate::lower_arg_list(
                            all_args,
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        let r = crate::lower_return_annotation(
                            method_def.returns.as_deref(),
                            method_is_public,
                            &method_name,
                            type_param.as_deref(),
                            Some(&class_name),
                            &[],
                            class_name_defs,
                        )?;
                        (p, r)
                    };
                // Protocol member signatures exclude `self` (already
                // stripped above).
                let param_tys: Vec<Ty> = params.iter().map(|(_, ty)| ty.clone()).collect();
                let member = ProtocolMember::Method {
                    name: method_name.clone(),
                    param_tys,
                    return_ty,
                };
                // Replace if redeclared, otherwise add.
                if let Some(existing) = protocol_members.iter().position(
                    |m| matches!(m, ProtocolMember::Method { name, .. } if name == &method_name),
                ) {
                    protocol_members[existing] = member;
                } else {
                    protocol_members.push(member);
                }
            }
            Stmt::AnnAssign(ann) => {
                let Expr::Name(target) = ann.target.as_ref() else {
                    return Err(unsupported(
                        "a protocol attribute annotation must target a bare name (`x: int`), \
                         not an attribute access, subscript, or other expression",
                        pycc_ast::expr_range(&ann.target),
                    ));
                };
                let attr_name = target.id.to_string();
                let attr_ty = crate::annotation_to_ty(
                    &ann.annotation,
                    type_param.as_deref(),
                    Some(&class_name),
                    &[],
                    class_name_defs,
                )?;
                // A protocol attribute cannot have a default value.
                if ann.value.is_some() {
                    return Err(unsupported(
                        format!(
                            "a protocol attribute `{class_name}.{attr_name}` cannot have a \
                             default value -- protocol attributes are requirements, not \
                             initializers"
                        ),
                        ann.range,
                    ));
                }
                let member = ProtocolMember::Attribute {
                    name: attr_name.clone(),
                    ty: attr_ty,
                };
                if let Some(existing) = protocol_members.iter().position(
                    |m| matches!(m, ProtocolMember::Attribute { name, .. } if name == &attr_name),
                ) {
                    protocol_members[existing] = member;
                } else {
                    protocol_members.push(member);
                }
            }
            Stmt::Pass(_) => {
                // `pass` is a no-op in a protocol body.
            }
            // #744: a docstring (a bare string-literal expression statement)
            // is a no-op, matching `validate_init_subclass_body`'s existing
            // precedent for the same construct.
            Stmt::Expr(expr_stmt) if matches!(*expr_stmt.value, Expr::StringLiteral(_)) => {}
            _ => {
                return Err(unsupported(
                    format!(
                        "a protocol class body must contain only method definitions (`def ...`) \
                         and annotated assignments (`x: int`) -- {:?} is not supported yet",
                        stmt
                    ),
                    pycc_ast::stmt_range(stmt),
                ));
            }
        }
    }
    Ok((
        HirClassDef {
            exception_type_tag: None,
            name: class_name,
            bases,
            mro,
            attrs: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: true,
            runtime_checkable,
            protocol_members,
            abstract_methods: Vec::new(),
            is_abstract: false,
        },
        Vec::new(),
    ))
}
