//! The `ext` boundary's type table: which pycc [`Ty`] the generated
//! wrapper can carry, what it occupies in C at a parameter or result
//! position, and the `C0003` refusal ([`EXT_CAPABILITY_CODE`]) for everything
//! else.
//!
//! Extracted from `src/ext_build.rs` when Part 1 of #1027 added the third
//! carrier (`AGENTS.md`'s decomposability rule: a task that touches an
//! oversized file decomposes the part it touches). The seam is cohesive on
//! its own terms -- every item here answers a question about a *type*,
//! while what remains in the parent module answers questions about a
//! toolchain, a link line and generated C text.

use super::EXT_CAPABILITY_CODE;
use pycc_diag::{Diagnostic, Severity};
use pycc_hir::Ty;

/// What one `Ty` the boundary admits occupies at a generated wrapper's
/// parameter or result position.
///
/// A scalar occupies one C slot. A `tuple` occupies one slot per element
/// and never a slot of its own (#1050): C cannot spell a pycc aggregate,
/// because pycc's own convention for passing and returning one is not the
/// platform C struct ABI. An enum rather than a `Vec` that happens to hold
/// one entry, so every consumer has to say which case it is handling --
/// this table is the entire seam between the driver's C and codegen's
/// LLVM, and a width or a slot count that disagrees with the callee is a
/// silent miscompile, never a compile error on either side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundaryCarrier {
    /// One C slot: its C type, and the `pycc_ext_*` helper suffix that
    /// unpacks and packs a value of it.
    Scalar(&'static str, &'static str),
    /// A `tuple`, as one `Scalar`'s payload per element in declaration
    /// order. D-116 fixes a tuple type's arity, so this is exactly its
    /// element list and never a run-time length.
    Tuple(Vec<(&'static str, &'static str)>),
    /// A one-dimensional `float` `memoryview` (Part 1 of #1027): one C slot
    /// holding a pointer to the wrapper's own [`BUFFER_VIEW_C_TYPE`] local,
    /// plus a `Py_buffer` the wrapper owns for the whole call. The only
    /// carrier that owes cleanup on *both* the bail path and the success
    /// path -- see [`BoundaryCarrier::cleanup`].
    ///
    /// `writable` selects the `PyObject_GetBuffer` request (Part 1 of
    /// #1142): `true` adds `PyBUF_WRITABLE` for a parameter the compiled
    /// body stores into, and `false` -- the only value [`boundary_carrier`]
    /// itself ever produces -- leaves the read-only request D-244's
    /// 2026-09-17 Part-1-of-#1027 amendment statement (c) established. It
    /// is a property of a function *body*, not of a [`Ty`], so it is set
    /// where each slot vector is built and never here.
    Buffer { writable: bool },
}

/// What one already-unpacked argument slot owes the wrapper before it
/// leaves, stated per carrier rather than by matching a helper suffix.
///
/// #1049 selected `str`'s bail-path `decref` by a string-literal match on
/// the helper suffix `"str"`. That worked while exactly one carrier owed
/// anything, but a `memoryview`'s `Py_buffer` is a second obligation *and*
/// a differently-shaped one (it is owed on the success path too), so the
/// selector is a property of the carrier now and a fourth class cannot be
/// added by matching a fourth literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotCleanup {
    /// The fresh `PyStrObj` reference `pycc_ext_unpack_str` produced, which
    /// only the compiled function's own parameter slot ever consumes.
    StrDecref,
    /// The `Py_buffer` `pycc_ext_unpack_memoryview` acquired, which the
    /// wrapper must release on every exit after acquisition -- including
    /// the one where the compiled call returned normally.
    BufferRelease,
}

impl BoundaryCarrier {
    /// The single C slot this carrier occupies, or `None` for a `tuple`,
    /// which occupies several.
    pub(crate) fn into_scalar(self) -> Option<(&'static str, &'static str)> {
        match self {
            BoundaryCarrier::Scalar(c_type, helper) => Some((c_type, helper)),
            BoundaryCarrier::Tuple(_) => None,
            // Not a scalar in the sense this accessor is asked about. The
            // two callers are `return_c_type` and the `tuple`-element
            // lookup in `boundary_carrier`, and a `memoryview` is admitted
            // at neither position: it cannot be returned (there is no
            // CPython object to hand back -- the wrapper released the
            // buffer it borrowed) and `tuple[memoryview]` has no `_at`
            // element shim. Answering `None` is what turns both into the
            // ordinary `C0003` capability gap.
            BoundaryCarrier::Buffer { .. } => None,
        }
    }

    /// What this carrier owes at an argument position, or `None` when it
    /// owes nothing.
    pub(crate) fn cleanup(&self) -> Option<SlotCleanup> {
        match self {
            BoundaryCarrier::Scalar(_, "str") => Some(SlotCleanup::StrDecref),
            // Every numeric scalar is a copied machine word, and a `tuple`'s
            // elements are copied out by value, so neither owes anything.
            BoundaryCarrier::Scalar(..) | BoundaryCarrier::Tuple(_) => None,
            BoundaryCarrier::Buffer { .. } => Some(SlotCleanup::BufferRelease),
        }
    }
}

/// The C type of the `{ void *ptr, long long len }` pair a `memoryview`
/// parameter crosses the boundary as, `typedef`'d in [`SHIM_C`] above the
/// point the generated companion is `#include`d at.
///
/// A pycc-owned two-word POD and deliberately not CPython's `Py_buffer`:
/// emitted IR must not depend on that struct's layout, and `Py_buffer.shape`
/// is exporter-owned storage valid only until `PyBuffer_Release`. The
/// wrapper copies `shape[0]` into this pair's `len` at acquisition, so
/// nothing downstream can dereference `shape` after the release.
pub(crate) const BUFFER_VIEW_C_TYPE: &str = "PyccExtBufferView";

/// The C slots one type the boundary admits uses inside a generated
/// wrapper, or `None` when this pycc version's boundary cannot carry `ty`
/// in either position.
///
/// Each C type is chosen to match exactly what `pycc_codegen`'s
/// `ty_to_basic_type` gives the compiled function, because the wrapper
/// reaches that function through a seam no compiler can check: `Ty::Int`
/// is `i64`, `Ty::Float` is `f64`, and `Ty::Bool` is a one-byte `i8`
/// holding `0`/`1` at the parameter position as well as the return one --
/// hence `char`, and deliberately not `int` or `_Bool`. A width that
/// disagrees with the callee is a silent miscompile here, never a compile
/// error.
///
/// `Ty::Str` is the one scalar entry that is not a number -- hence this
/// function's name, which #1049 widened from `boundary_scalar`.
/// `ty_to_basic_type` gives it an opaque pointer, so the C type is
/// `void *` in both positions and the wrapper never reads through it:
/// `pycc_ext_unpack_str` produces the `PyStrObj` and `pycc_ext_pack_str`
/// consumes it.
///
/// `Ty::Tuple` (#1050) is the one entry that is not a single slot at all.
/// Its elements are looked up through this same function, so their widths
/// come from the same table rather than a parallel one, but only D-116's
/// three element types are admitted: a tuple carrying anything else -- a
/// nested tuple, `tuple[list[int]]`, or `tuple[str, int]`, none of which
/// a `T0039`-checked program can express -- answers `None` exactly as any
/// other uncarriable type does.
pub(crate) fn boundary_carrier(ty: &Ty) -> Option<BoundaryCarrier> {
    match ty {
        Ty::Int => Some(BoundaryCarrier::Scalar("long long", "int")),
        Ty::Float => Some(BoundaryCarrier::Scalar("double", "float")),
        Ty::Bool => Some(BoundaryCarrier::Scalar("char", "bool")),
        Ty::Str => Some(BoundaryCarrier::Scalar("void *", "str")),
        Ty::Tuple(elements) => elements
            .iter()
            .map(|element| match element {
                // The one type the two admissibility questions answer
                // differently, and so the one arm `into_scalar` cannot
                // decide. `str` occupies a single C slot at a top-level
                // position (#1049), so it would pass `into_scalar`
                // unchanged -- but the element shims are the `_at`
                // variants, which take an element index and exist only
                // for D-116's three element types. Admitting
                // `tuple[str, int]` here would render C naming an
                // undeclared `pycc_ext_unpack_str_at`: a clang error on
                // the generated artifact instead of the `C0003`
                // capability gap every other uncarriable shape gets.
                // Unreachable from source today, exactly as
                // `tuple[list[int]]` is -- `T0039` refuses the
                // annotation -- which is why it is stated rather than
                // left to a recursion that happens to work.
                Ty::Str => None,
                // Every other element goes through this same function,
                // so its width comes from the table above rather than a
                // parallel one, and a `list` element (no carrier at all)
                // or a nested tuple (a carrier, but not a single slot)
                // answers `None` on its own.
                _ => boundary_carrier(element).and_then(BoundaryCarrier::into_scalar),
            })
            .collect::<Option<Vec<_>>>()
            .map(BoundaryCarrier::Tuple),
        // Part 1 of #1027: admitted at a parameter position only. The four
        // run-time refusals the boundary applies to the object itself --
        // exports a buffer, C-contiguous, `ndim == 1`, format `"d"` --
        // live in `pycc_ext_unpack_memoryview`, because none of them is a
        // property of the *declared* type this table answers about.
        //
        // Part 1 of #1142: `writable: false` is the *only* answer this
        // function gives. Writability is a property of the body that
        // receives the parameter, which a pure function of a `Ty` cannot
        // see -- `wrapper_for` and `tp_init_c` overwrite the flag from
        // `ExtExport`/`ExtCtor` where they build their slot vectors.
        Ty::MemoryView => Some(BoundaryCarrier::Buffer { writable: false }),
        _ => None,
    }
}

/// Whether the boundary can carry `ty` at a parameter position.
///
/// `Ty::None` is deliberately not admitted: a `None` parameter stays a
/// capability gap, gated on #1047's call-argument ICE. Everything else the
/// boundary carries at all, it carries in both directions, so this is
/// [`boundary_carrier`] with no further narrowing -- the asymmetry lives
/// entirely in [`return_c_type`].
pub(crate) fn carries_param(ty: &Ty) -> bool {
    boundary_carrier(ty).is_some()
}

/// The C return type of a wrapper's call into the compiled function, or
/// `None` when the boundary cannot carry `ty` as a return type.
///
/// Two types answer `void`, for different reasons. Codegen emits a `None`
/// return as LLVM `void`, so there is nothing to receive at all and the
/// wrapper's egress becomes `Py_RETURN_NONE`. A `tuple` return does carry
/// values, but they leave through `pycc_ext_thunk_<name>`'s trailing
/// out-pointers rather than as a return value (#1050), so the call itself
/// is still `void` and the elements are read out of the wrapper's own
/// locals afterwards.
pub(crate) fn return_c_type(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::None => Some("void"),
        // Still asked of `boundary_carrier`: `tuple[list[int]]` is a tuple
        // whose element the boundary cannot carry, and answering `void`
        // for it unconditionally would admit a signature no wrapper can
        // unpack.
        Ty::Tuple(_) => boundary_carrier(ty).map(|_| "void"),
        _ => boundary_carrier(ty)
            .and_then(BoundaryCarrier::into_scalar)
            .map(|(c_type, _)| c_type),
    }
}

/// Names the first part of a signature the `ext` boundary cannot carry, as
/// the user would write it (`x: str`, `-> list`), or `None` when the whole
/// signature is admissible.
///
/// Parameters and the return type are asked separately because the two
/// admissible sets genuinely differ rather than sharing one widened list:
/// see [`carries_param`] and [`return_c_type`].
pub(crate) fn unsupported_boundary_ty(params: &[(String, Ty)], return_ty: &Ty) -> Option<String> {
    if let Some((name, ty)) = params.iter().find(|(_, ty)| !carries_param(ty)) {
        return Some(format!("parameter `{name}: {}`", render_ty(ty)));
    }
    if return_c_type(return_ty).is_none() {
        return Some(format!("return type `-> {}`", render_ty(return_ty)));
    }
    None
}

/// A short Python-facing spelling of a `Ty`, for the `C0003` message only.
///
/// Deliberately coarse: the reader's fix is to change the signature, and
/// the element type of the container that was refused adds nothing to that.
/// The `Ty::Tuple(_)` arm survives #1050 rather than becoming dead, because
/// a `tuple` is admitted by its elements: `tuple[list[int]]` and a nested
/// `tuple[tuple[int], int]` still reach this function, and both are named
/// `tuple` -- the element that failed is the same one D-116 already forbids
/// spelling in a type annotation (T0039), so naming it would point at a
/// program that cannot be written.
pub(crate) fn render_ty(ty: &Ty) -> &'static str {
    match ty {
        Ty::Int => "int",
        Ty::Float => "float",
        Ty::Bool => "bool",
        Ty::Str => "str",
        Ty::None => "None",
        Ty::List(_) => "list",
        Ty::Dict(_) => "dict",
        Ty::Set(_) => "set",
        Ty::Tuple(_) => "tuple",
        // Part 1 of #1026: the spelling `Ty::name()` uses, so the gap
        // message names the same thing a `T0023` about the binding would.
        Ty::Object => "object",
        // Part 1 of #1027: reachable from a real signature, because a
        // `memoryview` *return* type is a capability gap while the
        // parameter position is admitted.
        Ty::MemoryView => "memoryview",
        _ => "that type",
    }
}

/// Builds the `C0003` diagnostic for one unexportable public function or
/// method.
///
/// Span-less: `HirItem::Function` carries no source range (the whole point
/// of `pycc_hir`'s lowered form), and `pycc_diag::render_human` renders a
/// span-less diagnostic as exactly `error[C0003]: <message>`, which is what
/// `report_build_failure` needs.
///
/// `name` is the **compiled** name, so for a method it arrives mangled
/// (`Grid.scale.static`). Both the subject and the remedy are rendered from
/// the source-level spelling instead: the subject reads `Grid.scale`, a
/// spelling the user actually wrote, and the remedy reads `Grid._scale` --
/// renaming the *method* private is what keeps it out of the export set.
/// `_Grid.scale` and `_Grid.scale.static` are both unusable, and the second
/// is what a naive `_{name}` renders.
pub(crate) fn capability_gap(name: &str, offender: &str) -> Diagnostic {
    let source_name = crate::ext_build::source_level_name(name);
    // `Grid.scale` -> ("Grid.", "scale"); `f` -> ("", "f"). The remedy
    // renames the last component, which is the method for a method and the
    // function itself for a module-level `def`.
    let (owner, member) = match source_name.rsplit_once('.') {
        Some((class, method)) => (format!("{class}."), method),
        None => (String::new(), source_name),
    };
    let noun = if owner.is_empty() {
        "function"
    } else {
        "method"
    };
    Diagnostic {
        code: EXT_CAPABILITY_CODE,
        severity: Severity::Error,
        message: format!(
            "--ext cannot export the public {noun} `{source_name}`: its {offender} is not a type \
             this pycc version's CPython boundary can carry -- a parameter must be `int`, \
             `float`, `bool`, `str`, `memoryview` (or its other spellings `ndarray` and \
             `NDArray`) or a \
             `tuple` of `int`/`float`/`bool`, and a \
             return type must be one of those except the buffer, or `None` \
             (D-244 rule \
             1 exports every public module-level function, and a public `@staticmethod` or \
             `@classmethod` of a public class, so there is no way to opt one \
             out) -- rename it to `{owner}_{member}` to keep it out of the export set, or build \
             without --ext"
        ),
        span: None,
        label: None,
        help: None,
    }
}
