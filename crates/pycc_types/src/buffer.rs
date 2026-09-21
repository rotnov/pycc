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
///
/// A `bool` is *not* such a case: it is an `int` by the representation table
/// (`docs/TYPE_SYSTEM.md`, rule 4/D-086), so `ndarray(True)` requests one
/// element. See [`producer_assignment_ty`]'s own length check, and
/// `crate::constraints::reject_non_int_producer_length` for the solver's
/// mirror of it -- whose absence made this refusal unreachable for an
/// unannotated private helper until #1165's review round 8.
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
    //
    // `local_names` is the fourth arm rather than a redundant one. It is a
    // whole-body pre-pass (`crate::function_local_names` = the parameters
    // plus every assignment, `for`, comprehension, `match` capture and
    // `except ... as` target `collect_local_names` reaches at any depth), so
    // it answers CPython's own scoping question: a function that binds the
    // spelling *anywhere* in its body makes it local *throughout*, and a use
    // before that binding is an `UnboundLocalError` rather than a producer.
    // Without this arm `a = ndarray(4)` followed by a later `ndarray = 1` in
    // the same body allocated, because `env.bindings` never holds a local
    // that has not been bound yet at this point in the walk. The solver's
    // mirror is `crate::constraints::resolved_producer_call`; both then let
    // the ordinary `Call` walk report the program's own error, which for a
    // syntactic local is the solver's `unbound_local` (`T0021`).
    //
    // `std_module_aliases` is the fifth arm for the same reason, and it is
    // the one arm that was a *miscompile* rather than a misdirected
    // message: `import math as ndarray` binds the spelling to the `math`
    // module, but a stdlib module alias is recorded only in that table --
    // never in `env.bindings` -- so without this arm `a = ndarray(4)`
    // allocated a buffer for a program whose own binding makes the call
    // CPython's `TypeError: 'module' object is not callable`. Declining
    // here hands it to the ordinary `Call` walk, which reports `T0021`
    // exactly as it does for an unaliased spelling. `docs/TYPE_SYSTEM.md`'s
    // `memoryview` row is the canonical enumeration of the binding kinds
    // statement (h) covers.
    if env.lookup_class(callee).is_some()
        || env.lookup_generic(callee).is_some()
        || env.lookup_function(callee).is_some()
        || env.bindings.contains_key(callee.as_str())
        || crate::is_local(local_names, callee)
        || env
            .std_module_aliases
            .iter()
            .any(|(alias, _)| alias == callee)
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
    // `bool` is admitted alongside `int`, exactly as it is for a buffer
    // *index* (`docs/TYPE_SYSTEM.md`'s `memoryview` row, rule 4/D-086):
    // `crate::is_assignable`'s own `from == Ty::Bool && to == Ty::Int` clause
    // encodes that representation-table subtyping, and codegen already
    // decodes the value -- `MirExpr::BufferAlloc`'s arm routes the length
    // through `to_numeric_encoded_int`, whose `Scalar::Bool` arm zero-extends
    // the `i8` to `i64` and re-tags it before the shared checked untag, so
    // `ndarray(True)` reaches the allocator as the element count `1`.
    //
    // Written as an explicit match rather than `is_assignable(len_ty,
    // Ty::Int)` because that helper also admits a `Ty::Param`, which is not a
    // length; the `float(...)` arm in `crate::expr` spells its own
    // `Int | Float | Bool` admission out for the same reason.
    if !matches!(len_ty, Ty::Int | Ty::Bool) {
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

/// The producer spellings `params`/`body` bind as function-local names, for
/// the same one consumer outside this crate as
/// [`is_buffer_producer_spelling`].
///
/// `src/memoryview_mode.rs`'s native-mode body gate has to reproduce
/// [`producer_assignment_ty`]'s statement-(h) decline *exactly*, and that
/// decline's fourth arm is `crate::is_local` over
/// `crate::function_local_names`. Exporting the composition of those two
/// private helpers -- rather than letting the gate re-derive locality with a
/// walk of its own -- is what makes the gate's skip sound by construction:
/// the gate skips a spelling only where this returns it, this returns
/// exactly what the checker's fourth arm tests, and where that arm declines
/// the checker never binds `Ty::MemoryView`, so no allocation the gate
/// stopped refusing can reach a native artifact. A hand-rolled locality test
/// could diverge, and in the one direction that matters.
///
/// Filtered to the producer spellings so the export stays as narrow as
/// [`is_buffer_producer_spelling`]: `function_local_names` itself and the
/// whole local-name vocabulary stay crate-private.
pub fn function_local_producer_spellings<'a>(
    params: &'a [(String, Ty)],
    body: &'a [pycc_hir::HirStmt],
) -> Vec<&'a str> {
    crate::function_local_names(params, body)
        .into_iter()
        .filter(|name| is_producer_spelling(name))
        .collect()
}

/// The producer spellings a module's own import table binds, for the same
/// one consumer outside this crate as [`function_local_producer_spellings`].
///
/// `import math as ndarray` binds the spelling to the `math` module, so
/// D-244 #1129 statement (h) makes it the program's own meaning and
/// [`producer_assignment_ty`] declines the call. The check phase reads that
/// fact from `crate::Environment`'s `std_module_aliases`, which
/// `crate::std_receiver::bind_std_module_aliases` fills;
/// `src/memoryview_mode.rs`'s native-mode body gate walks the HIR instead
/// and has no environment to ask, so this export gives it the same
/// computation rather than a second walk that agrees today.
///
/// The predicate is deliberately *exactly* `bind_std_module_aliases`' -- an
/// `ImportBinding::Module` whose `local_name` is not the module's canonical
/// spelling -- and must never be widened to the other `ImportBinding`
/// variants. Membership in that gate's shadow set means "skip the refusal",
/// so a set wider than the checker's decline set is under-refusal: an
/// artifact-owned allocation reaching a native executable.
/// `ImportBinding::Symbol` (`from math import sqrt as ndarray`) and
/// `ImportBinding::Foreign` (`import ndarray`) are refused upstream
/// (`C0001` and `I0404`), so neither needs an entry here.
pub fn imported_producer_spellings(imports: &[pycc_hir::ImportBinding]) -> Vec<&str> {
    imports
        .iter()
        .filter_map(|binding| match binding {
            pycc_hir::ImportBinding::Module { local_name, module }
                if local_name != pycc_std::module_name(*module) =>
            {
                Some(local_name.as_str())
            }
            _ => None,
        })
        .filter(|name| is_producer_spelling(name))
        .collect()
}

/// The name an admitted buffer **egress** returns, or `None` when this
/// `return` is not the one shape Part 2b of #1142 (#1164) admits.
///
/// The shape is deliberately exact: a bare name, in a function whose
/// *declared* return type is the buffer type. Both walkers call this from
/// their own `HirStmt::Return` arm, before the operand is ever inferred, on
/// the same **interception** model `crate::expr::reject_memoryview_read`'s
/// doc comment records for `b[i]` and `len(b)` -- the refusal itself is not
/// weakened, only the set of expressions that reach it narrows by one.
///
/// Membership in the caller's own `owned_buffers` set is the caller's half
/// of the test and is deliberately *not* asked here: that set lives on two
/// different environments (the check phase's [`crate::Environment`] and the
/// solver's `ConstraintEnvironment`), and leaving the provenance question
/// with each walker is what keeps a *parameter*-bound name -- `return b` --
/// falling through to the parameter refusal, which is the use-after-free
/// #1142 exists to forbid.
///
/// A declared return type other than the buffer type declines here, so
/// `def f(n: int) -> float: a = ndarray(n); return a` keeps exactly the
/// refusal it has today.
pub(crate) fn admitted_buffer_return<'a>(
    expr: &'a pycc_hir::HirExpr,
    declared_return: Option<&Ty>,
) -> Option<&'a str> {
    match (expr, declared_return) {
        (pycc_hir::HirExpr::Name(name), Some(Ty::MemoryView)) => Some(name.as_str()),
        _ => None,
    }
}

/// `Err(C0001)` for a call whose callee returns the buffer type.
///
/// Part 2b of #1142 (#1164) admits a buffer return at the `pycc build --ext`
/// boundary -- where the generated wrapper turns the artifact's storage into
/// a real `memoryview` the host owns -- and nowhere else. An *intra-artifact*
/// call to such a function has no such wrapper: the compiled callee hands
/// back a raw `PyccExtBufferView *` with no owner, which
/// `crates/pycc_codegen/src/call_result.rs`'s `Ty::MemoryView` arm panics on.
/// This refusal is what keeps that panic unreachable from source, which is
/// #1164's own completion criterion.
pub(crate) fn buffer_returning_call_unsupported(callee: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "calling `{callee}`, whose return type is a buffer, is valid Python but not \
             implemented yet; #1164 hands such a buffer to the CPython host across the \
             `pycc build --ext` boundary and admits no intra-artifact caller -- allocate \
             the buffer with `a = ndarray(n)` in the function that reads it"
        ),
        Span::new(0, 0),
    )
}

/// `Err(C0001)` for a buffer **egress** in a function that also contains a
/// `return` inside a `finally` clause (Part 2b of #1142, #1164 review
/// round 5).
///
/// The narrowing that makes the single pending-return record's own
/// precondition checked rather than assumed. `crates/pycc_codegen/src/lib.rs`
/// tracks a returned buffer's ownership in **one** per-frame record -- a
/// pointer slot plus an orphan flag -- which can describe exactly one
/// suspended return. A `return` lexically inside a `finally` is the only
/// shape that puts two returns in flight at once: the outer `return` is
/// suspended while its finalizer runs, the inner one overwrites the record
/// and releases the orphaned predecessor, and a finalizer that then raises
/// cancels the inner return so the outer one resumes -- handing the host a
/// `memoryview` over freed storage and segfaulting the interpreter.
///
/// Refusing the *admission* rather than growing the record is deliberate.
/// Four review rounds on this mechanism each closed one path into it; the
/// cardinality assumption underneath them is what this closes, and the
/// whole class with it. A correct multi-pending egress needs a stack of
/// records keyed by suspended-return context, which is tracked separately.
///
/// Raised only at the egress admission, so a `-> int` function with a
/// `return` inside a `finally` keeps exactly the behavior it has today:
/// this narrows what buffer egress admits, and nothing else.
///
/// pycc already refuses a *bare* `return` inside a `finally` with `L0001`
/// (PEP 765, #738). That check is syntactic and follows CPython in clearing
/// its `finally` context on loop entry, so `while True: return a` escapes
/// it; `pycc_hir::body_returns_inside_finally` is the transitive predicate
/// this refusal uses instead. `C0001` is the code because this is a
/// capability gap in an unimplemented feature, not a context violation --
/// the same ground as every other refusal in this module (D-148).
pub(crate) fn buffer_return_inside_finally(name: &str) -> Diagnostic {
    Diagnostic::error(
        "C0001",
        format!(
            "returning `{name}`, which is bound to buffer storage this `pycc build --ext` \
             artifact allocated, from a function that also contains a `return` inside a \
             `finally` clause is valid Python but not implemented yet; #1164 tracks one \
             pending buffer return per call, and a `return` inside a `finally` can leave \
             a second one suspended -- move the inner `return` out of the `finally` \
             clause"
        ),
        Span::new(0, 0),
    )
}
