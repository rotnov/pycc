//! The artifact-owned buffer producer and the refusals that bound it
//! (Part 2a of #1142, issue #1165).
//!
//! Until this module existed, `Ty::MemoryView` had exactly one source: a
//! parameter of a `pycc build --ext` export, borrowed from the host for the
//! duration of one call. `crate::expr::reject_memoryview_read`'s own doc
//! comment states the consequence that fact bought -- "`memoryview` has no
//! literal and no producing expression, so ... [every other position] is
//! reachable only through that read and is refused by it at once".
//!
//! This module adds the second source: `ndarray(n)` / `NDArray(n)` in
//! expression position, yielding a zero-filled one-dimensional `float64`
//! buffer of `n` elements that the artifact itself allocates and frees. With
//! two sources, "which refusal applies to this name" stops being a question
//! about the *type* and becomes a question about the *binding*, which is why
//! [`crate::Environment`] and `crate::constraints::ConstraintEnvironment`
//! both carry an `owned_buffers` set and
//! [`crate::expr::reject_memoryview_read`] takes a provenance flag.
//!
//! # The producer's admitted position
//!
//! Exactly one: the entire right-hand side of an assignment -- bare or
//! annotated -- to a simple local name, inside a function body. Every other
//! syntactic position is refused by [`producer_position_unsupported`], and
//! module scope by [`producer_at_module_scope`].
//!
//! That single-position rule is what makes Part 2a's free-at-function-exit
//! lifetime sound: the value cannot be aliased, stored, returned, passed or
//! left on the floor, so the allocating frame is provably the only owner and
//! `crates/pycc_codegen/src/call_result.rs`'s `Ty::MemoryView` panic stays
//! unreachable by construction rather than by luck.
//!
//! # Where the refusal lives, and why it is one message
//!
//! Both type walkers refuse the producer spelling from their own `Call` arm
//! and both admit it only from their own assignment arm. Refusing in the
//! `Call` arm is what reaches the positions a `Name`-keyed refusal
//! structurally cannot: `crate::check_stmt_in_function`'s `HirStmt::ExprStmt`
//! arm discards the inferred type, so a bare `ndarray(4)` statement would
//! otherwise type-check, lower to a `MirStmt::ExprStmt` that
//! `collect_stmt_bindings` never gives a slot, and leak one allocation per
//! call in a long-lived host process.
//!
//! The plan for #1165 asked for a distinct message per refused position
//! (call argument, container literal, return value, comprehension, subscript
//! base, bare statement). One named producer-position diagnostic raised from
//! the `Call` arm is used instead: the plan's actual objection is that those
//! positions are refused *incidentally* today (`f(ndarray(3))` falls out as a
//! parameter-type mismatch), and a named refusal raised before the value can
//! reach any of them removes the incidentality for all of them at once,
//! without six messages that can drift apart. Each position keeps its own
//! pin in the test matrix.
//!
//! # Spelling, and whose binding wins
//!
//! `ndarray` and `NDArray` -- the two spellings #1129 and #1134 already gave
//! the *annotation*, now callable. `memoryview` is deliberately **not** a
//! producer spelling: `memoryview(5)` is a `TypeError` in CPython while
//! `numpy.ndarray(5)` is a real constructor, so admitting it would mean
//! something CPython refuses outright rather than something it does
//! differently. D-244's #1134 "one type, three spellings" is therefore
//! qualified rather than broken -- it still holds for every rule stated
//! about the *type*, and the call position is the first rule stated about a
//! *spelling*.
//!
//! D-244's #1129 statement (h) governs the call position identically to the
//! annotation position: a program's own `class ndarray` or `def NDArray`
//! wins. In the check phase that falls out of placement -- the interception
//! sits inside `infer_expr_in`'s `lookup_function` `else` block, after the
//! class and generic lookups have already run. The solver has no class
//! table, so `ConstraintEnvironment::shadowed_producers` carries the same
//! fact explicitly; see its own doc comment.

use pycc_diag::{Diagnostic, Span};
use pycc_hir::Ty;

/// The two callable spellings of the buffer producer.
///
/// Deliberately *not* registered in `crate::is_known_callable_builtin` or in
/// `pycc_hir::typecheck::is_builtin_type_name`: either registration would
/// make `isinstance(x, ndarray)` an accept, against D-244's #1129
/// statement (e).
pub(crate) const PRODUCER_SPELLINGS: [&str; 2] = ["ndarray", "NDArray"];

/// Whether `callee` is spelled like the buffer producer.
///
/// A spelling match alone decides nothing: the caller must still apply
/// statement (h) (the program's own binding wins) and the arity rule before
/// treating the call as a producer.
pub(crate) fn is_producer_spelling(callee: &str) -> bool {
    PRODUCER_SPELLINGS.contains(&callee)
}

/// `Err(C0001)` for a buffer producer anywhere but an assignment's
/// right-hand side.
///
/// See this module's own doc comment for why one message covers every
/// refused position.
pub(crate) fn producer_position_unsupported(callee: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "`{callee}(n)` here is valid Python but not implemented yet; a buffer \
             producer is admitted only as the whole right-hand side of an assignment \
             to a local name inside a `pycc build --ext` export, as in `a = {callee}(n)`"
        ),
        Span::new(0, 0),
    )
}

/// `Err(C0001)` for a buffer producer at module scope.
///
/// Its own refusal rather than a case of [`producer_position_unsupported`]
/// because the ground is different, and Part 2b will lift exactly one of the
/// two. A module-level frame gets no epilogue at all
/// (`crates/pycc_codegen/src/lib.rs`'s owned-slot frame is skipped for
/// `main` / `__pycc_ext_exec`), and the "cannot escape its function"
/// invariant that makes free-at-exit sound says nothing about a value that
/// was never in a function.
pub(crate) fn producer_at_module_scope(callee: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "`{callee}(n)` at module scope is valid Python but not implemented yet; \
             artifact-owned buffer storage is freed when the allocating function \
             returns, and module-level code has no such frame -- allocate it inside a \
             `pycc build --ext` export instead"
        ),
        Span::new(0, 0),
    )
}

/// `Err(T0033)` when a buffer producer's length argument is not an `int`.
pub(crate) fn producer_length_not_an_int(callee: &str, len_ty: &Ty) -> Diagnostic {
    Diagnostic::error(
        "T0033",
        format!(
            "`{callee}` expects an `int` element count, got `{}`",
            len_ty.name()
        ),
        Span::new(0, 0),
    )
    .with_help("pass an `int` length, as in `a = ndarray(4)`")
}

/// `Err(C0001)` for any use of an **artifact-owned** buffer beyond the three
/// operations Part 2a implements.
///
/// The owned counterpart of [`crate::expr::reject_memoryview_read`]'s
/// parameter arm, whose message is pinned verbatim by eight assertions and
/// stays exactly as it was. The distinction is load-bearing rather than
/// cosmetic: `return a` on an owned name is refused because egress does not
/// exist yet (Part 2b), while `return b` on a parameter-bound name is
/// refused because handing back a view the host lent for one call is a
/// use-after-free. Reporting the parameter message for the owned case would
/// leave Part 2b inheriting a refusal that lies about why it exists.
pub(crate) fn owned_buffer_use_unsupported(name: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "using `{name}`, which is bound to buffer storage this `pycc build --ext` \
             artifact allocated, is valid Python but not implemented yet; #1165 admits \
             such a buffer only inside the function that allocated it, read one element \
             at a time with `{name}[i]` over `range(len({name}))`, and store one with \
             `{name}[i] = 1.0`"
        ),
        Span::new(0, 0),
    )
}

/// `Err(C0001)` for assigning to a name bound to a buffer **parameter**.
///
/// Refused in Part 2a for two independent reasons, either of which is
/// sufficient. The memory-safety one: `crates/pycc_codegen/src/lib.rs`
/// pushes parameter slots into the frame's owned-slot list on the `str`
/// precedent, so a rebound buffer parameter would make the epilogue `free`
/// a `PyccExtBufferView *` the host owns. The behavioral one:
/// `pycc_hir::body_stores_into` is a syntactic walk matching
/// `HirStmt::DictSet` targets against *parameter names*, so a store into the
/// rebound local would make `src/ext_build.rs` request `PyBUF_WRITABLE` on
/// the host's buffer for a store the body never performs on it.
///
/// Refusing this shape is also what makes the flat, flow-insensitive
/// provenance model sound: no name can be parameter-bound for the first half
/// of a function and artifact-owned for the second.
pub(crate) fn buffer_parameter_rebinding(name: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "assigning to `{name}`, a buffer parameter of a `pycc build --ext` export, \
             is valid Python but not implemented yet; the wrapper borrows that buffer \
             from the host for exactly one call -- bind the new buffer to a different \
             local name instead"
        ),
        Span::new(0, 0),
    )
}

/// The check phase's one admitting seam for the buffer producer.
///
/// `None` means "`value` is not a buffer producer here" and the caller falls
/// through to ordinary inference; `Some(Ok(Ty::MemoryView))` means the
/// caller must bind `target` as artifact-owned storage; `Some(Err(..))` is a
/// refusal that belongs to the producer rather than to the assignment.
///
/// Called from exactly the four assignment arms that admit it -- module and
/// function scope, plain and annotated -- so "the producer's admitted
/// position" is one call site per arm rather than a property scattered
/// across the expression walk. Everything this function declines is refused
/// by `infer_expr_in`'s own `Call` arm.
///
/// Statement (h) is applied here explicitly, because unlike the `Call` arm
/// this seam runs *before* the class, generic and function lookups: a
/// program's own `class ndarray`, `def ndarray`, or module-level `ndarray =
/// ...` all make this return `None`, leaving the ordinary walk to resolve
/// the call as the program meant it.
pub(crate) fn producer_assignment_ty(
    env: &crate::Environment,
    local_names: &[&str],
    value: &pycc_hir::HirExpr,
) -> Option<Result<Ty, Diagnostic>> {
    let pycc_hir::HirExpr::Call { callee, args } = value else {
        return None;
    };
    if !is_producer_spelling(callee) {
        return None;
    }
    // D-244 #1129 statement (h): the program's own binding wins, in call
    // position exactly as in annotation position.
    if env.lookup_class(callee).is_some()
        || env.lookup_generic(callee).is_some()
        || env.lookup_function(callee).is_some()
        || env.bindings.contains_key(callee.as_str())
    {
        return None;
    }
    // A wrong arity is not a producer at all: it falls through to the `Call`
    // arm, whose refusal names the admitted shape (`a = ndarray(n)`).
    if args.len() != 1 {
        return None;
    }
    if !env.in_function_body {
        return Some(Err(producer_at_module_scope(callee)));
    }
    let len_ty = match crate::expr::infer_expr_in(env, local_names, &args[0]) {
        Ok(ty) => ty,
        Err(diagnostic) => return Some(Err(diagnostic)),
    };
    if !matches!(len_ty, Ty::Int) {
        return Some(Err(producer_length_not_an_int(callee, &len_ty)));
    }
    Some(Ok(Ty::MemoryView))
}

/// The public spelling predicate, for the one consumer outside this crate.
///
/// `src/memoryview_mode.rs`'s native-mode body gate has to recognize the
/// producer *before* the type check runs (it is a mode refusal, and neither
/// `pycc_hir` nor this crate carries artifact-mode awareness), so it cannot
/// ask the typed result and must match the spelling itself. Exporting the
/// predicate keeps `PRODUCER_SPELLINGS` the one place the two spellings
/// are written down for the checker side of the compiler; `pycc_mir`'s own
/// lowering guard names them literally and cross-references this constant,
/// because `pycc_mir` does not depend on this crate.
pub fn is_buffer_producer_spelling(callee: &str) -> bool {
    is_producer_spelling(callee)
}
