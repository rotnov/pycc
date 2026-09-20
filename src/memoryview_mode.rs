//! The native-mode gate on a `memoryview` annotation (Part 1 of #1027).
//!
//! A `memoryview` parameter is a borrowed view of storage a CPython
//! interpreter owns: the value only exists because a generated
//! `pycc build --ext` wrapper acquired a `Py_buffer` from a real
//! `memoryview` object and released it again when the call returned
//! (`src/ext_build.rs`'s wrapper generator owns that discipline). A plain
//! `pycc build` produces a standalone native executable with no interpreter
//! at all, so there is nothing to acquire a buffer *from*, and the program
//! is refused here rather than compiled into a signature nothing could ever
//! call.
//!
//! This is a sibling of `src/foreign_import.rs`, and for the same reason:
//! `crates/pycc_hir` and `crates/pycc_types` have no artifact-mode
//! awareness at all, so a refusal that depends on the mode cannot live in
//! either. `src/frontend.rs`'s `resolve_frontend_native` is the one seam
//! that knows.
//!
//! It refuses no miscompile, and that is recorded rather than left for a
//! later reader to rediscover as dead weight: Part 1 adds no expression
//! that *produces* a `memoryview` value (the constructor is still refused),
//! so no call site inside a native program could type-check against such a
//! signature in the first place. The gate exists to make the refusal
//! legible -- a named code, a sentence naming `--ext`, and a test pinning
//! both -- instead of leaving a `memoryview` annotation to fail later with
//! a message about something else.

use pycc_diag::{Diagnostic, Severity};
use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt, ProtocolMember, Ty};
use std::collections::{HashMap, HashSet};

/// The `I04xx` code this gate emits. The family is the CPython interop
/// boundary (`docs/DIAGNOSTICS.md`): `I0403` is its nearest neighbour --
/// a CPython *import* in native mode -- and this is a CPython *buffer* in
/// native mode.
pub(crate) const NATIVE_MEMORYVIEW_CODE: &str = "I0405";

/// One [`NATIVE_MEMORYVIEW_CODE`] per function whose signature names
/// `memoryview`, paired with that function's index in `hir.items`, or
/// `Ok(())` when the program names it nowhere.
///
/// Shaped like `src/ext_build.rs`'s `collect_exports` rather than like
/// `src/foreign_import.rs`'s import walk: a `memoryview` annotation is not
/// an entry in the flat import table, it is a `Ty` on an `HirItem::Function`
/// in `hir.items`. Every offending function is reported, not only the
/// first, so one build names the whole list.
///
/// Span-less, exactly like `src/ext_build.rs`'s `capability_gap`:
/// `HirItem::Function` carries no source range -- that is the point of
/// `pycc_hir`'s lowered form -- and `pycc_diag::render_human` renders a
/// span-less diagnostic as plain `error[I0405]: <message>`. The item index
/// still names the owning *file*, which is what `src/frontend.rs` joins it
/// against.
///
/// Private functions are gated too, unlike the `--ext` export set: the
/// refusal is about the artifact having no interpreter, which a `_`-prefixed
/// name does not change.
///
/// `src/frontend.rs`'s `resolve_frontend_native` reports these gaps *before*
/// the type check, unlike the foreign-import gate beside it: otherwise a body
/// that reads its own `memoryview` parameter fails
/// `crates/pycc_types`'s `reject_memoryview_read` first and the signature
/// never gets the code it is documented to get. That call site owns the
/// reasoning.
///
/// A *signature* is the whole of this gate's job. `pycc_hir`'s
/// `annotation_to_ty` also parses a bare `x: memoryview` declaration, which
/// this walk never reaches -- it visits `params` and `return_ty`, not
/// statement bodies -- and which no artifact mode admits at all;
/// `crates/pycc_types`'s `reject_memoryview_declaration` refuses that
/// position with `C0001` in both modes.
pub(crate) fn refuse_in_native_mode(hir: &HirModule) -> Result<(), Vec<(usize, Diagnostic)>> {
    let gaps: Vec<(usize, Diagnostic)> = hir
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let HirItem::Function {
                name,
                params,
                return_ty,
                ..
            } = item
            else {
                return None;
            };
            let position = offending_position(params, return_ty)?;
            Some((index, gap(name, &position)))
        })
        .collect();
    if gaps.is_empty() {
        return Ok(());
    }
    Err(gaps)
}

/// The protocol-method half of [`refuse_in_native_mode`]: one
/// [`NATIVE_MEMORYVIEW_CODE`] per protocol method whose *parameter* list
/// names `memoryview`, paired with that class's index in `hir.class_defs`.
///
/// It needs its own walk because a protocol method is never lowered to an
/// `HirItem::Function` -- `crates/pycc_hir/src/class/protocol.rs` records it
/// as a `ProtocolMember::Method` and says so in its header -- so the
/// `hir.items` walk above cannot see the signature at all, and a
/// `class P(Protocol): def total(self, v: memoryview) -> int: ...` built
/// natively escaped the documented `I0405` entirely.
///
/// Only the parameter position is checked here, and that split is the whole
/// design: a `memoryview` *return* type is unsatisfiable in every artifact
/// mode, so `protocol.rs` refuses it at the declaration with `C0001` for
/// both modes at once, while a `memoryview` *parameter* is mode-dependent --
/// under `pycc build --ext` a class really can satisfy it, because an
/// exported wrapper acquires the buffer and passes the view inward (measured:
/// a `def total(self, v: memoryview) -> int` method builds under `--ext`
/// today). Native mode is exactly where it becomes impossible, which is this
/// gate's own subject.
///
/// Keyed by class index rather than item index, so the caller resolves it
/// through `ProgramSources::owner_of_class` instead of `owner_of_item`:
/// `class_defs` is its own concatenated table with its own per-file bounds,
/// and a protocol class defined in an imported module is contributed by that
/// module, not by the entry file.
///
/// An *inherited* member is skipped. `lower_protocol_class` copies a base
/// protocol's members into the derived class's own `protocol_members`, so a
/// `class Q(P, Protocol)` carries `P`'s offending method verbatim; reporting
/// it again would name `Q` for a signature `P` declares. The declaring class
/// is the one that gets the diagnostic, exactly as for the return position.
///
/// "Inherited" means the *whole member* matches an ancestor's, not merely
/// its name. A derived protocol may override an ancestor's method with a
/// different signature -- `lower_protocol_class` replaces the copied entry
/// when the body redeclares the name -- so `class P(Protocol): def f(self,
/// x: int)` followed by `class Q(P): def f(self, v: memoryview)` leaves `Q`
/// as the declaring class of an offending signature that `P` never had.
/// Comparing by name alone would skip it and let the native build through.
///
/// Span-less and parameter-name-less, both for the same reason as
/// [`gap`]: `ProtocolMember::Method` carries `param_tys: Vec<Ty>` and no
/// names at all, so the position is named by its 1-based index with `self`
/// already stripped -- the shape `protocol.rs` records.
pub(crate) fn refuse_protocol_methods_in_native_mode(
    hir: &HirModule,
) -> Result<(), Vec<(usize, Diagnostic)>> {
    let by_name: HashMap<&str, &Vec<ProtocolMember>> = hir
        .class_defs
        .iter()
        .map(|(name, def)| (name.as_str(), &def.protocol_members))
        .collect();
    let mut gaps: Vec<(usize, Diagnostic)> = Vec::new();
    for (index, (class_name, def)) in hir.class_defs.iter().enumerate() {
        for member in &def.protocol_members {
            let ProtocolMember::Method {
                name, param_tys, ..
            } = member
            else {
                continue;
            };
            let Some(position) = param_tys.iter().position(|ty| *ty == Ty::MemoryView) else {
                continue;
            };
            if def.mro.iter().any(|ancestor| {
                ancestor != class_name && declares_member(&by_name, ancestor, member)
            }) {
                continue;
            }
            gaps.push((
                index,
                gap(
                    &format!("{class_name}.{name}"),
                    &format!("parameter {} `memoryview`", position + 1),
                ),
            ));
        }
    }
    if gaps.is_empty() {
        return Ok(());
    }
    Err(gaps)
}

/// Whether `class_name` names a protocol class that declares `member`
/// itself -- the same name *and* the same signature, so that the copy in
/// the derived class really is the ancestor's and not an override of it.
/// A name the class table does not hold (a builtin base, `Protocol`
/// itself) declares nothing.
fn declares_member(
    by_name: &HashMap<&str, &Vec<ProtocolMember>>,
    class_name: &str,
    member: &ProtocolMember,
) -> bool {
    by_name
        .get(class_name)
        .is_some_and(|members| members.iter().any(|m| m == member))
}

/// Names the first part of a signature that mentions the buffer type, in
/// its canonical spelling, or `None` when it mentions it nowhere.
///
/// The rendered spelling is `memoryview` whichever of the three source
/// spellings the user wrote (#1129 admits `ndarray` as the second, #1134
/// `NDArray` as the third), for the reason `Ty::MemoryView`'s own
/// documentation gives: one type, one canonical name, exactly as for an
/// alias of it.
///
/// Both positions are checked, and only `Ty::MemoryView` itself matches,
/// with no recursion into a container's elements. A parameterized container
/// annotation does lower its elements through the same `annotation_to_ty`
/// recursion, so `list[memoryview]` -- and, since #1129, `list[ndarray]` --
/// really does produce `Ty::List(Ty::MemoryView)`. What keeps that type out
/// of a lowered function's `params` and `return_ty` is the capability gate
/// one step later: `pycc_hir`'s `check_container_ty` admits only `Ty::Int`
/// as a list element and rejects every other one with `T0034`. So the
/// nested shape is unreachable here by a type-directed refusal, not by a
/// name that fails to resolve.
fn offending_position(params: &[(String, Ty)], return_ty: &Ty) -> Option<String> {
    if let Some((name, _)) = params.iter().find(|(_, ty)| *ty == Ty::MemoryView) {
        return Some(format!("parameter `{name}: memoryview`"));
    }
    if *return_ty == Ty::MemoryView {
        return Some("return type `-> memoryview`".to_string());
    }
    None
}

/// The `--ext` counterpart: one `C0001` per function whose *return* type is
/// `memoryview`, or `Ok(())` when none is.
///
/// `src/ext_build.rs`'s `collect_exports` already refuses that return type
/// on a *public* function, and on a public `@staticmethod` or
/// `@classmethod` of a public non-exception class, with `C0003` and the
/// export-boundary wording. This closes the rest of the program: a private
/// `def _make() -> memoryview`, an instance method, a property, a method of
/// a private or exception class, or a monomorphized specialization is
/// skipped by that walk entirely, so nothing refused the signature and
/// lowering the call's result reached `pycc_codegen`'s "a
/// `memoryview`-typed call result is not supported yet" panic -- a compiler
/// crash on valid Python, not a diagnostic. The `C0003` message's own
/// advice ("rename it to `_name` to keep it out of the export set") pointed
/// straight at that crash.
///
/// **A public `memoryview`-returning `@staticmethod` or `@classmethod` of a
/// public class now reports `C0003` instead of the `C0001` this walk used
/// to give it.** That is the export boundary's own diagnostic taking over a
/// signature it now owns, not a new refusal: the program was already
/// rejected, and only the code and the wording change. It also means such a
/// method's `C0003` *suppresses* the `C0001`s this walk would have reported
/// in the same run, because `plan_ext` aborts on `collect_exports`' gaps
/// before reaching here. That arm is newly reachable rather than new: the
/// same suppression has always applied to a public module-level function's
/// `C0003`, and `C0003`'s own "every gap in a program is reported at once"
/// is a statement about the export-boundary gap set, which is collected in
/// full before the first is reported.
///
/// `C0001` rather than `C0003`: `docs/DIAGNOSTICS.md` defines `C0003` as a
/// *public* function's signature failing to cross the boundary, which a
/// private one is not. This is the same versioned capability gap
/// `crates/pycc_types`'s `reject_memoryview_declaration` and
/// `reject_memoryview_read` report, for the same underlying reason: neither
/// #1027 nor #1129 adds an expression that *produces* a buffer, so there is
/// nothing a function could return. The message is worded like those two
/// siblings, and for the same reason: it names the type as "a buffer"
/// rather than in any of its three spellings (#1134 added `NDArray`), so a
/// user who wrote `ndarray` or `NDArray` is not told about a `memoryview`
/// they never mentioned. The
/// canonical-spelling rendering `offending_position` does is the *native*
/// path's, whose `I0405` quotes a whole signature position back; this
/// message quotes no annotation text at all.
///
/// Only the return position is checked. A `memoryview` *parameter* on a
/// private function carries no value into codegen either way: for the same
/// reason there is nothing to return, there is nothing to pass, so no call
/// to such a function can be written at all -- whether its body reads the
/// parameter with the `b[i]` load Part 2 of #1027 admits or is refused a
/// `C0001` for reading it any other way. And native mode is not this
/// function's concern --
/// [`refuse_in_native_mode`] refuses both positions there, private
/// functions included.
///
/// Called after `collect_exports`, so an offender that is *in the export
/// set* -- a public module-level function, or a public `@staticmethod` or
/// `@classmethod` of a public non-exception class -- has already been
/// reported as the `C0003` its documented boundary owes it and never
/// reaches this walk.
pub(crate) fn refuse_in_ext_mode(hir: &HirModule) -> Result<(), Vec<Diagnostic>> {
    let gaps: Vec<Diagnostic> = hir
        .items
        .iter()
        .filter_map(|item| {
            let HirItem::Function {
                name, return_ty, ..
            } = item
            else {
                return None;
            };
            (*return_ty == Ty::MemoryView).then(|| ext_return_gap(name))
        })
        .collect();
    if gaps.is_empty() {
        return Ok(());
    }
    Err(gaps)
}

fn ext_return_gap(name: &str) -> Diagnostic {
    Diagnostic {
        code: "C0001",
        severity: Severity::Error,
        message: format!(
            "`{name}`'s return type is a buffer, which is valid Python but not implemented \
             yet; Part 1 of #1027 and #1129 admit a buffer only as a parameter of a \
             `pycc build --ext` export, so no expression produces one to return"
        ),
        span: None,
        label: None,
        help: None,
    }
}

/// One [`NATIVE_MEMORYVIEW_CODE`] per function body that binds
/// artifact-owned buffer storage, paired with that function's index in
/// `hir.items`, or `Ok(())` when the program allocates none (Part 2a of
/// #1142, issue #1165).
///
/// The third native-mode buffer gate, and the first that reads *bodies*
/// rather than signatures. It exists because #1165 gave `Ty::MemoryView` a
/// second source that the two signature walks above structurally cannot
/// see: `a = ndarray(n)` names the type nowhere.
///
/// Its ground is deliberately *not* [`gap`]'s. A `memoryview` parameter is
/// impossible natively -- there is no interpreter to acquire a `Py_buffer`
/// from. Artifact-owned storage is not: `pycc_rt_buffer_f64_alloc` is a
/// plain heap allocation and `pycc_rt` links into a native executable
/// unchanged, so a native `a = ndarray(4)` would *run*. It is refused
/// because the buffer type exists to carry data across the `pycc build
/// --ext` boundary, and admitting a second, interpreter-free meaning for it
/// in native mode would commit the project to that meaning before anything
/// needs it. Reusing [`gap`]'s wording here would state a reason that is
/// false of this case, so [`producer_gap`] states the real one.
///
/// Only the *admitted* producer shape is searched, and that is sufficient
/// rather than approximate: `crates/pycc_types/src/buffer.rs` admits a
/// producer call in exactly one position -- the whole right-hand side of an
/// assignment, bare or annotated -- and refuses it with `C0001` in every
/// other position, in every artifact mode. A producer this walk does not
/// find is therefore already refused by the checker behind it.
///
/// D-244's #1129 statement (h) is honoured with the same three-part shadow
/// set the checker uses (`lookup_class`, `lookup_function`, `env.bindings`),
/// read from the HIR: a program's own `class ndarray`, `def ndarray`, or
/// module-level `ndarray = ...` keeps its own meaning and is not reported.
/// A value-less module-level `ndarray: int` is not such a binding; see
/// [`shadowed_producer_spellings`].
/// Over-refusal is the failure mode that matters here -- it rejects a legal
/// program -- so the set is widened to the whole module rather than scoped
/// per function.
///
/// `pycc check` selects no artifact mode and so does not run this gate, the
/// same deliberate divergence [`refuse_in_native_mode`] already has: the
/// refusal's whole subject is *which artifact* is being built.
pub(crate) fn refuse_buffer_producers_in_native_mode(
    hir: &HirModule,
) -> Result<(), Vec<(usize, Diagnostic)>> {
    let shadowed = shadowed_producer_spellings(hir);
    let gaps: Vec<(usize, Diagnostic)> = hir
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let HirItem::Function { name, body, .. } = item else {
                return None;
            };
            let callee = producer_bound_in(body, &shadowed)?;
            Some((index, producer_gap(name, callee)))
        })
        .collect();
    if gaps.is_empty() {
        return Ok(());
    }
    Err(gaps)
}

/// The producer spellings the program itself rebinds, which
/// [`refuse_buffer_producers_in_native_mode`] must leave alone.
///
/// A module-level `AnnAssign` counts only when it carries an initializer.
/// `HirStmt::AnnAssign`'s `value` is an `Option`, and a value-less `ndarray:
/// int` binds nothing at run time -- the check phase records it in `declared`
/// rather than in `Environment::bindings`, so its statement-(h) test does not
/// see it either and `buffer::producer_assignment_ty` still recognizes the
/// producer. Treating the bare annotation as a shadow here while the checker
/// does not was a one-sided disagreement in the only direction that matters:
/// this gate skipped its refusal, and an artifact-owned buffer allocation
/// reached a **native** executable, past the `--ext`-only boundary
/// [`producer_gap`] exists to state.
fn shadowed_producer_spellings(hir: &HirModule) -> HashSet<&str> {
    let mut shadowed: HashSet<&str> = HashSet::new();
    for (name, _) in &hir.class_defs {
        if pycc_types::is_buffer_producer_spelling(name) {
            shadowed.insert(name.as_str());
        }
    }
    for item in &hir.items {
        let name = match item {
            HirItem::Function { name, .. } => name,
            HirItem::TopLevelStmt(HirStmt::Assign { target, .. })
            | HirItem::TopLevelStmt(HirStmt::AnnAssign {
                target,
                value: Some(_),
                ..
            }) => target,
            HirItem::TopLevelStmt(_) => continue,
        };
        if pycc_types::is_buffer_producer_spelling(name) {
            shadowed.insert(name.as_str());
        }
    }
    shadowed
}

/// The spelling of the first producer call `body` binds to a name at any
/// nesting depth, or `None` when it binds none.
///
/// Exhaustive with no `_` arm for the same reason
/// `crates/pycc_hir/src/buffer_store.rs`'s walk is: a future block-carrying
/// `HirStmt` variant must be a compile error here rather than a hole this
/// gate silently stops covering.
fn producer_bound_in<'a>(body: &'a [HirStmt], shadowed: &HashSet<&str>) -> Option<&'a str> {
    body.iter().find_map(|stmt| match stmt {
        HirStmt::Assign { value, .. } => producer_callee(Some(value), shadowed),
        HirStmt::AnnAssign { value, .. } => producer_callee(value.as_ref(), shadowed),
        HirStmt::If { body, orelse, .. } => {
            producer_bound_in(body, shadowed).or_else(|| producer_bound_in(orelse, shadowed))
        }
        HirStmt::While { body, .. }
        | HirStmt::ForRange { body, .. }
        | HirStmt::ForList { body, .. }
        | HirStmt::ForObject { body, .. } => producer_bound_in(body, shadowed),
        HirStmt::Match { cases, .. } => cases
            .iter()
            .find_map(|case| producer_bound_in(&case.body, shadowed)),
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
        } => producer_bound_in(body, shadowed)
            .or_else(|| {
                handlers
                    .iter()
                    .find_map(|handler| producer_bound_in(&handler.body, shadowed))
            })
            .or_else(|| producer_bound_in(orelse, shadowed))
            .or_else(|| producer_bound_in(finalbody, shadowed)),
        // Every remaining statement either carries no nested block or binds
        // no name from a call expression, so no *admitted* producer can hide
        // in one; a producer anywhere inside them is the checker's `C0001`.
        HirStmt::ExprStmt(_)
        | HirStmt::ListCompAssign { .. }
        | HirStmt::DictCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::Return(_)
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        | HirStmt::Raise { .. } => None,
    })
}

/// The producer spelling `value` calls, honouring `shadowed` and the
/// one-argument arity the checker admits.
fn producer_callee<'a>(value: Option<&'a HirExpr>, shadowed: &HashSet<&str>) -> Option<&'a str> {
    let HirExpr::Call { callee, args } = value? else {
        return None;
    };
    if args.len() != 1
        || shadowed.contains(callee.as_str())
        || !pycc_types::is_buffer_producer_spelling(callee)
    {
        return None;
    }
    Some(callee.as_str())
}

fn producer_gap(name: &str, callee: &str) -> Diagnostic {
    Diagnostic {
        code: NATIVE_MEMORYVIEW_CODE,
        severity: Severity::Error,
        message: format!(
            "`{name}` allocates buffer storage with `{callee}(n)`, which requires \
             `pycc build --ext`: a buffer exists to carry data across the CPython \
             extension-module boundary, and a native executable has no host to carry \
             it to"
        ),
        span: None,
        label: None,
        help: None,
    }
}

fn gap(name: &str, position: &str) -> Diagnostic {
    Diagnostic {
        code: NATIVE_MEMORYVIEW_CODE,
        severity: Severity::Error,
        message: format!(
            "`{name}`'s {position} requires `pycc build --ext`: a `memoryview` is a borrowed \
             view of storage a CPython interpreter owns, and a native executable embeds no \
             interpreter to acquire a buffer from"
        ),
        span: None,
        label: None,
        help: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hir(items: Vec<HirItem>) -> HirModule {
        HirModule {
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    fn func(name: &str, params: Vec<(String, Ty)>, return_ty: Ty) -> HirItem {
        HirItem::Function {
            name: name.to_string(),
            params,
            return_ty,
            body: Vec::new(),
        }
    }

    #[test]
    fn a_program_without_a_memoryview_annotation_is_admitted() {
        let admitted = hir(vec![func(
            "scale",
            vec![("x".to_string(), Ty::Float)],
            Ty::Float,
        )]);
        assert!(refuse_in_native_mode(&admitted).is_ok());
    }

    #[test]
    fn ext_mode_refuses_a_memoryview_return_type_on_a_function_no_export_walk_visits() {
        let gaps = refuse_in_ext_mode(&hir(vec![
            // A `memoryview` parameter is not this gate's business: the
            // read refusal in `crates/pycc_types` already closes it.
            func("total", vec![("v".to_string(), Ty::MemoryView)], Ty::Int),
            // A non-function item is skipped: only a signature can name a
            // return type at all.
            HirItem::TopLevelStmt(pycc_hir::HirStmt::Assign {
                target: "n".to_string(),
                value: pycc_hir::HirExpr::IntLiteral(1),
            }),
            func("_make", Vec::new(), Ty::MemoryView),
            func("Buf.view", Vec::new(), Ty::MemoryView),
        ]))
        .expect_err("a `memoryview` return type is refused under --ext");
        let messages: Vec<&str> = gaps.iter().map(|gap| gap.message.as_str()).collect();
        assert_eq!(gaps.len(), 2, "{messages:?}");
        assert!(
            gaps.iter()
                .all(|gap| gap.code == "C0001" && gap.span.is_none()),
            "{messages:?}"
        );
        assert!(
            messages[0].contains("`_make`'s return type is a buffer"),
            "{messages:?}"
        );
        assert!(
            messages[1].contains("`Buf.view`'s return type is a buffer"),
            "{messages:?}"
        );
    }

    #[test]
    fn ext_mode_admits_a_program_that_returns_no_memoryview() {
        let admitted = hir(vec![func(
            "total",
            vec![("v".to_string(), Ty::MemoryView)],
            Ty::Float,
        )]);
        assert!(refuse_in_ext_mode(&admitted).is_ok());
    }

    #[test]
    fn every_memoryview_signature_is_reported_not_only_the_first() {
        let gaps = refuse_in_native_mode(&hir(vec![
            func("total", vec![("xs".to_string(), Ty::MemoryView)], Ty::Float),
            func("plain", vec![("x".to_string(), Ty::Int)], Ty::Int),
            // The return position is the second half of the gate: it is a
            // `C0003` capability gap under `--ext`, but in native mode it
            // is the same refusal a parameter gets.
            func("view", Vec::new(), Ty::MemoryView),
        ]))
        .expect_err("a memoryview annotation is refused in native mode");
        let messages: Vec<&str> = gaps.iter().map(|(_, gap)| gap.message.as_str()).collect();
        assert_eq!(gaps.len(), 2, "{messages:?}");
        assert!(
            gaps.iter()
                .all(|(_, gap)| gap.code == NATIVE_MEMORYVIEW_CODE && gap.span.is_none()),
            "{messages:?}"
        );
        assert!(messages[0].contains("`xs: memoryview`"), "{messages:?}");
        assert!(messages[1].contains("`-> memoryview`"), "{messages:?}");
        assert!(messages[0].contains("--ext"), "{messages:?}");
        // Each gap carries its own index in `hir.items` -- not its position
        // among the offending ones -- because that is what the driver joins
        // against the program's per-file item bounds to name the owning file.
        let positions: Vec<usize> = gaps.iter().map(|(index, _)| *index).collect();
        assert_eq!(positions, vec![0, 2], "{messages:?}");
    }

    // ---------------------------------------------------------------------
    // Part 2a of #1142 (#1165): the body-level artifact-owned buffer gate.
    // ---------------------------------------------------------------------

    /// A minimal non-generic, non-protocol class, for the statement-(h)
    /// shadow test below.
    fn plain_class_def(name: &str) -> pycc_hir::HirClassDef {
        pycc_hir::HirClassDef {
            class_attrs: Vec::new(),
            exception_type_tag: None,
            name: name.to_string(),
            bases: Vec::new(),
            mro: vec![name.to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            type_param: None,
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            is_enum: false,
            implicit_object_init: true,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        }
    }

    /// `a = <callee>(4)`.
    fn alloc(callee: &str) -> HirStmt {
        HirStmt::Assign {
            target: "a".to_string(),
            value: HirExpr::Call {
                callee: callee.to_string(),
                args: vec![HirExpr::IntLiteral(4)],
            },
        }
    }

    /// `def <name>(): <body>`.
    fn body_func(name: &str, body: Vec<HirStmt>) -> HirItem {
        HirItem::Function {
            name: name.to_string(),
            params: vec![],
            return_ty: Ty::None,
            body,
        }
    }

    /// Both spellings, both binding forms, and a message whose ground is the
    /// missing *host* rather than the missing interpreter: the allocation
    /// itself would work natively, which is exactly why the refusal needs
    /// its own wording instead of [`gap`]'s.
    #[test]
    fn allocating_a_buffer_natively_is_refused_in_its_own_words() {
        for stmt in [
            alloc("ndarray"),
            alloc("NDArray"),
            HirStmt::AnnAssign {
                target: "a".to_string(),
                annotation: Ty::MemoryView,
                value: Some(HirExpr::Call {
                    callee: "ndarray".to_string(),
                    args: vec![HirExpr::IntLiteral(4)],
                }),
                is_final: false,
            },
        ] {
            let module = hir(vec![body_func("f", vec![stmt])]);
            let gaps = refuse_buffer_producers_in_native_mode(&module).unwrap_err();
            assert_eq!(gaps.len(), 1);
            assert_eq!(gaps[0].0, 0);
            assert_eq!(gaps[0].1.code, NATIVE_MEMORYVIEW_CODE);
            assert!(gaps[0].1.message.contains("`f`"), "{:?}", gaps[0].1);
            assert!(gaps[0].1.message.contains("pycc build --ext"));
            assert!(gaps[0].1.message.contains("extension-module boundary"));
            assert!(gaps[0].1.span.is_none());
        }
    }

    /// A program that allocates nowhere is untouched, and so is a top-level
    /// statement (module scope is `pycc_types`' own refusal, in every mode).
    #[test]
    fn a_program_that_allocates_no_buffer_is_admitted_natively() {
        assert!(refuse_buffer_producers_in_native_mode(&hir(vec![])).is_ok());
        assert!(
            refuse_buffer_producers_in_native_mode(&hir(vec![
                body_func("f", vec![HirStmt::Return(None)]),
                HirItem::TopLevelStmt(alloc("ndarray")),
            ]))
            .is_ok()
        );
    }

    /// Every nesting shape the walk recurses through reaches the producer,
    /// and every shape it deliberately does not recurse through is left to
    /// the checker's own `C0001`.
    #[test]
    fn the_walk_reaches_a_producer_at_every_nesting_depth() {
        let nested: Vec<Vec<HirStmt>> = vec![
            vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![alloc("ndarray")],
                orelse: vec![],
            }],
            vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![],
                orelse: vec![alloc("ndarray")],
            }],
            vec![HirStmt::While {
                test: HirExpr::BoolLiteral(true),
                body: vec![alloc("ndarray")],
            }],
            vec![HirStmt::Try {
                body: vec![alloc("ndarray")],
                handlers: vec![],
                orelse: vec![],
                finalbody: vec![],
            }],
            vec![HirStmt::Try {
                body: vec![],
                handlers: vec![],
                orelse: vec![alloc("ndarray")],
                finalbody: vec![],
            }],
            vec![HirStmt::Try {
                body: vec![],
                handlers: vec![],
                orelse: vec![],
                finalbody: vec![alloc("ndarray")],
            }],
            vec![HirStmt::Try {
                body: vec![],
                handlers: vec![pycc_hir::HirExceptHandler {
                    exc_type: None,
                    name: None,
                    body: vec![alloc("ndarray")],
                }],
                orelse: vec![],
                finalbody: vec![],
            }],
            vec![HirStmt::Match {
                subject: HirExpr::IntLiteral(0),
                cases: vec![pycc_hir::HirMatchCase {
                    pattern: pycc_hir::HirPattern::Wildcard,
                    guard: None,
                    body: vec![alloc("ndarray")],
                }],
            }],
            vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
                body: vec![alloc("ndarray")],
            }],
            // A value-less annotated declaration binds no producer, and a
            // nested block that holds none leaves the walk at `None`.
            vec![
                HirStmt::AnnAssign {
                    target: "a".to_string(),
                    annotation: Ty::Int,
                    value: None,
                    is_final: false,
                },
                alloc("ndarray"),
            ],
        ];
        for body in nested {
            let module = hir(vec![body_func("f", body.clone())]);
            assert!(
                refuse_buffer_producers_in_native_mode(&module).is_err(),
                "{body:?}"
            );
        }
        // Not an assignment's right-hand side, so not this gate's subject.
        let module = hir(vec![body_func(
            "f",
            vec![
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "ndarray".to_string(),
                    args: vec![HirExpr::IntLiteral(4)],
                }),
                HirStmt::Return(None),
            ],
        )]);
        assert!(refuse_buffer_producers_in_native_mode(&module).is_ok());
    }

    /// The arity the checker admits is the arity this gate claims: a
    /// two-argument call is `pycc_types`' `C0001`, not an `I0405`.
    #[test]
    fn a_producer_with_the_wrong_arity_is_not_this_gates_subject() {
        let module = hir(vec![body_func(
            "f",
            vec![HirStmt::Assign {
                target: "a".to_string(),
                value: HirExpr::Call {
                    callee: "ndarray".to_string(),
                    args: vec![HirExpr::IntLiteral(4), HirExpr::IntLiteral(5)],
                },
            }],
        )]);
        assert!(refuse_buffer_producers_in_native_mode(&module).is_ok());
    }

    /// D-244 #1129 statement (h): the program's own binding wins, through
    /// each of the three tables the checker consults. Over-refusal is the
    /// failure that matters here -- it rejects a legal native program.
    #[test]
    fn a_program_that_shadows_the_spelling_is_not_refused() {
        let own_function = hir(vec![
            HirItem::Function {
                name: "ndarray".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(None)],
            },
            body_func("f", vec![alloc("ndarray")]),
        ]);
        assert!(refuse_buffer_producers_in_native_mode(&own_function).is_ok());

        let own_binding = hir(vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "ndarray".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            body_func("f", vec![alloc("ndarray")]),
        ]);
        assert!(refuse_buffer_producers_in_native_mode(&own_binding).is_ok());

        let own_annotated_binding = hir(vec![
            HirItem::TopLevelStmt(HirStmt::AnnAssign {
                target: "ndarray".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
                is_final: false,
            }),
            body_func("f", vec![alloc("ndarray")]),
        ]);
        assert!(refuse_buffer_producers_in_native_mode(&own_annotated_binding).is_ok());

        let mut own_class = hir(vec![body_func("f", vec![alloc("NDArray")])]);
        own_class
            .class_defs
            .push(("NDArray".to_string(), plain_class_def("NDArray")));
        assert!(refuse_buffer_producers_in_native_mode(&own_class).is_ok());

        // An unrelated top-level statement contributes no shadow and stops
        // no refusal.
        let unrelated = hir(vec![
            HirItem::TopLevelStmt(HirStmt::Return(None)),
            body_func("f", vec![alloc("ndarray")]),
        ]);
        assert!(refuse_buffer_producers_in_native_mode(&unrelated).is_err());
    }

    /// Every function that allocates is named, not only the first: one build
    /// reports the whole list, exactly as the signature walk does.
    #[test]
    fn every_allocating_function_is_reported() {
        let module = hir(vec![
            body_func("f", vec![alloc("ndarray")]),
            body_func("g", vec![HirStmt::Return(None)]),
            body_func("h", vec![alloc("NDArray")]),
        ]);
        let gaps = refuse_buffer_producers_in_native_mode(&module).unwrap_err();
        assert_eq!(
            gaps.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![0, 2]
        );
    }
}
