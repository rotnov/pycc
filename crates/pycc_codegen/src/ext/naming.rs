//! The C-legal spelling of a compiled name at the `ext` seam and which
//! names it exports: [`mangle_ext_name`], [`ext_thunk_symbol`] and
//! [`is_ext_exportable_name`]. Split out of `ext.rs` (AGENTS.md's "Keep
//! source files decomposable") to bring it under the ~1,000-line threshold;
//! `ext.rs` re-exports every public item, so the public paths are unchanged.

use super::EXT_THUNK_PREFIX;

/// The C-legal spelling of a possibly-dotted pycc name.
///
/// A method reaches MIR under a dotted name (`Grid.scale.static`), and two
/// of the places that name is used are *C identifiers*: the
/// `extern void *fnptr_<name>;` declaration `pycc::ext_build`'s
/// `wrapper_for` emits, and the `pycc_ext_wrap_<name>` /
/// `pycc_ext_thunk_<name>` symbols. `extern void *fnptr_Grid.scale.static;`
/// is not accepted by any C compiler, so the dotted spelling has to be
/// encoded -- and `src/ext_build.rs`'s rebind-dedup comment records that a
/// *duplicate* export name makes clang reject the generated `.inc`
/// outright, so the encoding has to be injective as well as legal.
///
/// The encoding: a name with no `.` is returned unchanged; otherwise the
/// result is `"0m"` followed by, for each `.`-separated segment in order,
/// the segment's byte length in decimal, then `"_"`, then the segment. So
/// `f` stays `f` and `Grid.scale.static` becomes `0m4_Grid5_scale6_static`.
///
/// **It is injective, and that is argued rather than fixture-tested.**
/// Every dot-free name reaching this function is either a Python identifier
/// or carries the compiler-generated `0gen_` prefix
/// (`pycc_types::monomorphize`), and neither can begin with `0m` -- the
/// premise is stated this way rather than as "Python identifiers cannot
/// begin with a digit", because a dot-free name like `0gen_make__T_int` is
/// a real identity-branch input that *does* begin with a digit. So the
/// identity branch's outputs never collide with a `0m...` output. Within
/// the `0m` branch the encoding is length-prefixed and therefore uniquely
/// decodable, so two distinct dotted names cannot mangle alike. `.` -> `_`
/// and `.` -> `__` both fail this argument, the second one silently: a
/// module-level `def Grid__scale__static` would collide with
/// `Grid.scale.static`.
///
/// Every use site prefixes the result (`fnptr_`, `fnname_`,
/// `pycc_ext_thunk_`, `pycc_ext_wrap_`), so the leading digit never starts
/// a C identifier. Because the dot-free case is the identity, every symbol
/// the compiler emitted before methods became exportable is byte-identical.
///
/// This is the one canonical implementation: `pycc::ext_build` calls it
/// rather than reimplementing it, exactly as it already calls
/// [`ext_thunk_symbol`].
#[must_use]
pub fn mangle_ext_name(name: &str) -> String {
    if !name.contains('.') {
        return name.to_string();
    }
    let mut out = String::from("0m");
    for segment in name.split('.') {
        out.push_str(&segment.len().to_string());
        out.push('_');
        out.push_str(segment);
    }
    out
}

/// The external symbol `name`'s scalar-only `ext` export thunk is emitted
/// under.
///
/// `name` is mangled through [`mangle_ext_name`] first, so a method's thunk
/// is a legal C identifier the generated `extern` declaration can name. The
/// mangling is the identity for a dot-free name, so every module-level
/// function's thunk symbol is unchanged.
#[must_use]
pub fn ext_thunk_symbol(name: &str) -> String {
    format!("{EXT_THUNK_PREFIX}{}", mangle_ext_name(name))
}

/// Whether `name` is a name D-244 rule 1 can export at all, disregarding
/// its signature.
///
/// The tests are exactly `pycc::ext_build::collect_exports`' own *lexical*
/// verdict, and this is their one canonical home so the two sides of the
/// seam cannot drift: a wrapper generated for a name codegen declined to
/// emit a thunk for links cleanly and crashes on the first call.
/// `src/ext_build_tests/exports.rs` carries a test pinning the two equal
/// over a shared table of names, because the drift is otherwise silent --
/// `wrapper_for` picks the thunk `extern` or the `fnptr_` `extern` from
/// [`ext_thunk_required`](super::ext_thunk_required), so a disagreement emits the wrong C declaration
/// for a `tuple`-carrying method.
///
/// Two name classes are answered `true` beyond that lexical verdict: the
/// PEP 562 module hooks `__getattr__` and `__dir__` (#1467), which the
/// driver admits through `is_module_hook`, and `<Class>.<slot dunder>`
/// (#1427), which it admits through `is_slot_dunder_method`, neither through
/// `classify_export_name`. So the parity test compares this function with
/// `classify_export_name(name).is_some() || is_module_hook(name) ||
/// is_slot_dunder_method(name)` -- the last for #1427's comparison and
/// `__hash__` slots, admitted for any non-empty class segment.
///
/// The public-name test is D-038's predicate, spelled out rather than
/// delegated to `pycc_hir::is_public_name` because this crate deliberately
/// does not depend on `pycc_hir` -- it sees only `pycc_mir`'s re-export of
/// `Ty`. The body there is `!name.starts_with('_')` and nothing else; if it
/// ever grows a case, this copy must grow with it.
///
/// **Dotted names are no longer refused wholesale.** A method reaches MIR
/// under `<Class>.<method>` and the suffixed spellings
/// `<Class>.<method>.static`, `<Class>.<method>.classmethod` and
/// `<Class>.<property>.setter` (`pycc_hir::class`'s mangling). This admits
/// the `.static` and `.classmethod` spellings and -- since #1145 -- the
/// bare `<Class>.<method>` one, with every segment public.
/// `<Class>.<property>.setter` stays refused: a `@property` is attribute
/// syntax on the host side, never a method table entry.
///
/// The bare spelling covers the three `MethodKind`s `Regular`,
/// `PropertyGetter` and `AbstractMethod` at once, and nothing here can tell
/// them apart. Admitting it is deliberate rather than an approximation:
/// widening this mirror is what makes it a **superset** of the driver's
/// admitted set again, and a superset is the safe direction. The driver
/// narrows the bare spelling back down with filters that read
/// `HirModule::class_defs`; leaving this side refusing it would instead
/// make `ext_thunk_required` answer `false` for a `tuple`-carrying instance
/// method, so the wrapper would emit the `fnptr_` cast form -- measured to
/// fault (SIGBUS) on aarch64-apple-darwin for an out-pointer signature.
///
/// **This verdict is purely lexical, and must stay so.** The function
/// receives a bare `&str` and this crate cannot see `pycc_hir`, so a
/// verdict that consulted `HirModule::class_defs` would have no
/// mirror-comparable form here and the parity test would stop being
/// well-formed. The driver layers its exception-class exclusion *on top of*
/// this verdict rather than inside it, which makes this mirror a
/// **superset** of the driver's admitted set: at worst a thunk is emitted
/// for a name no wrapper calls, which is dead code -- never the link error
/// the drift above would be.
///
/// A monomorphized generic specialization carries the `0gen_` prefix and
/// has no `fnptr_` global to dispatch through, so it stays refused; that
/// test is applied to the whole name *before* the split and takes
/// precedence, as cheap defense in depth. It is not a live hazard: the real
/// specialization shapes put the substitution suffix last
/// (`0gen_<Class>.<method>__<P>_<C>`), so a `0gen_` name's last segment is
/// never `static`.
///
/// A receiver-exact copy of an inherited body (#1337, D-254) needs no case
/// of its own: a primary copy is spelled exactly as the receiver's own
/// definition would be (`C.m`, `C.k.classmethod`) and is judged as one, and
/// a `super()`-target copy (`C.m.0super_D`, plus a kind suffix for a setter
/// or classmethod) carries a third segment that is
/// neither `static` nor `classmethod`, so it is refused here and by the
/// driver alike -- it is only ever called from another copy, never hosted.
#[must_use]
pub fn is_ext_exportable_name(name: &str) -> bool {
    if name.starts_with("0gen_") {
        return false;
    }
    // #1467: the two PEP 562 module hooks, which the driver publishes from
    // the entry module although D-038's predicate refuses every dunder. The
    // driver admits only the entry module's own `def`, which this lexical
    // mirror cannot see, so a helper module's hook gets a dead thunk -- the
    // safe superset direction described above.
    if name == "__getattr__" || name == "__dir__" {
        return true;
    }
    if is_slot_dunder_method_name(name) {
        return true;
    }
    let mut segments = name.split('.');
    // `str::split` always yields at least one segment, so the fallback is
    // unreachable rather than a second refusal path; an empty first segment
    // is refused on the next line either way, which is what the driver's
    // mirror does with its own empty-segment guard.
    let first = segments.next().unwrap_or("");
    if first.starts_with('_') || first.is_empty() {
        return false;
    }
    let Some(second) = segments.next() else {
        // A dot-free name: a module-level function, admitted by D-038's
        // predicate alone.
        return true;
    };
    if second.starts_with('_') || second.is_empty() {
        return false;
    }
    match segments.next() {
        // `<Class>.<method>` -- `Regular`, `PropertyGetter` or
        // `AbstractMethod`, indistinguishable here and all admitted as the
        // superset the driver narrows (#1145).
        None => true,
        Some(kind) => {
            // The only four-segment name is a `super()`-target copy of a
            // setter or classmethod (`C.k.0super_D.classmethod`, D-254),
            // which is never hosted: a class nested in a class or a
            // function is refused by `pycc_hir` (`stmt.rs`, `class.rs`), so
            // no `A.B.method` name exists. Refusing every fourth segment is
            // the fail-closed reading.
            segments.next().is_none() && (kind == "static" || kind == "classmethod")
        }
    }
}

/// The special methods the `--ext` driver installs as a type slot rather
/// than a `PyMethodDef` row (#1427): the six rich comparisons and
/// `__hash__`. Mirrors `src/ext_build/richcompare.rs`'s `SLOT_DUNDERS`;
/// the parity test in `src/ext_build_tests/exports.rs` keeps the two equal.
const SLOT_DUNDERS: [&str; 7] = [
    "__lt__", "__le__", "__eq__", "__ne__", "__gt__", "__ge__", "__hash__",
];

/// #1427: whether `name` is `<Class>.<slot dunder>` -- a comparison or
/// `__hash__` method the driver wraps for a `tp_richcompare`/`tp_hash`
/// slot. The class segment may be private: an unpublished class whose
/// instance crosses gets a carrier type carrying the same slots, so its
/// wrapper needs the thunk exactly as a published one's does. A `0gen_`
/// name is refused by the caller before this runs.
fn is_slot_dunder_method_name(name: &str) -> bool {
    let mut segments = name.split('.');
    let class = segments.next().unwrap_or("");
    let Some(method) = segments.next() else {
        return false;
    };
    !class.is_empty() && segments.next().is_none() && SLOT_DUNDERS.contains(&method)
}
