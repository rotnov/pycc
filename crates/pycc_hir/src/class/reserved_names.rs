//! Class-attribute names the interpreter reserves for itself, rejected in
//! every class body (#910, #975).
//!
//! Extracted from [`super::attrs`] under `AGENTS.md`'s "keep source files
//! decomposable" rule when #975 added the second name set below, because
//! `attrs.rs` is already past the ~1000-line bar.
//!
//! Two independent sets live here:
//!
//! * `__slots__` (#910), which Python reads as a declaration of the instance
//!   layout. This compiler fixes that layout at compile time from `__init__`
//!   (D-154), so the declaration would be silently discarded. Its message is
//!   route-dependent ([`slots_message`]): an `Enum` body has no `__init__` and
//!   no instance layout at all, and CPython's `_EnumDict` turns the name into
//!   an ordinary class attribute there, so D-154's explanation would be false
//!   on that route. Since #980 the name has a *third* account, which
//!   deliberately lives outside [`slots_message`]: on the `@property` getter
//!   route the failure is not about the instance layout at all, so
//!   [`reject_reserved_property_name`] carries [`PROPERTY_SLOTS_MESSAGE`]
//!   instead. That route is not a [`ClassBodyRoute`] variant, because only
//!   one class body can reach it -- see the scope note below.
//! * The *instantiation and class-creation protocol* names (#975, D-236):
//!   `__init__`, `__new__` and `__init_subclass__`. The generating rule is
//!   "the names Python's object protocol calls implicitly rather than by
//!   name" -- `C()` runs `type.__call__` -> `__new__` -> `__init__`, and
//!   `class B(C)` runs `__init_subclass__` at class creation. This compiler
//!   resolves each protocol without ever consulting a class attribute of that
//!   name, so binding one of them silently changes behavior: for `__init__`
//!   and `__new__` that is a *measured* divergence -- CPython raises
//!   `TypeError` at the use site (`'int' object is not callable` for an `int`
//!   binding) while pycc compiled and ran the program, the D-198 false
//!   acceptance D-224 forbids. `__init_subclass__` is rejected
//!   **conservatively** instead, and D-236 records why: in an `Enum` body it
//!   can never diverge (an `Enum` with members cannot be subclassed, and
//!   `class C(Enum): __init_subclass__ = 1; B = 2` prints `2` under both
//!   engines), and in a plain body it diverges only once the class is actually
//!   subclassed -- whole-program information the class-body walk does not have
//!   when the guard fires. It is rejected on both routes for uniformity of the
//!   single rule, not because every binding was measured to fail.
//!
//! Scope notes that are easy to get wrong, all measured at `28a1b194`:
//!
//! * The guard is reached from **four** class-body routes, not three: the
//!   annotated and bare class-attribute paths, the `Enum` member loop, and
//!   (since the #978 review round) `super::body`'s `@property` getter arm.
//!   The fourth route goes through [`reject_reserved_property_name`], which
//!   covers the protocol names *and*, since #980, `__slots__` under its own
//!   message. It takes no [`ClassBodyRoute`]: a plain and a `@dataclass` body
//!   share that one arm, and an `Enum` body rejects method definitions
//!   outright before it (`C0001`, "an enum class body must contain only
//!   member assignments"), so an enum arm there would be a dead match arm and
//!   would fail D-014's 100% region gate.
//! * The set is **not** `ClassVar`-gated. `super::body`'s non-dataclass branch
//!   routes every `AnnAssign` to `lower_class_attr` regardless of the
//!   `ClassVar` wrapper, and #910's `Stmt::Assign` arm routes to
//!   `lower_unannotated_class_attr`, so `X: ClassVar[int] = 8`, `X: int = 8`
//!   and `X = 8` all reach the same hazard.
//! * The set deliberately does **not** include the other five names in
//!   `super::body`'s `DATACLASS_IMPLICIT_DUNDERS` (`__eq__`, `__repr__`,
//!   `__ne__`, `__str__`, `__format__`). Those are safe outside a dataclass
//!   *only because* every rewrite consulting them is `is_dataclass`-gated
//!   today (`pycc_mir/src/class.rs`'s early return, and `pycc_mir/src/expr.rs`'s
//!   `&& class_def.is_dataclass`). Ungating any of them for a plain class
//!   reintroduces the divergence and brings that name into this set.
//! * Conversely this set is checked in *every* class body, so it also closes
//!   the two names `DATACLASS_IMPLICIT_DUNDERS` omits (`__new__`,
//!   `__init_subclass__`) on the dataclass path, without disturbing the six
//!   messages that set already owns: `super::body` runs its own check first.
//! * The `Enum`-body `__slots__` rejection is conservative too, but only in
//!   the narrow sense. CPython 3.13.9 *accepts* `class C(Enum): __slots__ =
//!   "x"` with `A = 1` (the `class` statement succeeds, `C.__slots__` is
//!   `'x'`, `list(C)` is `[C.A]`, and the member still has a `__dict__`), so
//!   the rejection does refuse a program CPython runs. It is
//!   not conservative in the weaker sense of "nothing would have gone wrong":
//!   without the guard pycc lowers the name as a member, so an all-`str` enum
//!   (`__slots__ = "x"` alongside `A = "y"`, which CPython leaves at one
//!   member) would have compiled to a two-member enum. `__slots__ = ()` is the
//!   shape where no divergence is reachable, because a non-literal member
//!   value is rejected anyway.
//! * `__init_subclass__`'s rejection is conservative on two different
//!   grounds. In an `Enum` body it can never diverge, because an `Enum` with
//!   members cannot be subclassed at all. In a plain class body it diverges
//!   only if the class is actually subclassed somewhere, which is
//!   whole-program information this class-body walk does not have. It is
//!   rejected in both for uniformity of the single rule, not because a
//!   divergence was measured at the declaration site.

use crate::unsupported;
use pycc_diag::Diagnostic;

/// Rejects a class-body binding of a name the interpreter gives its own
/// meaning, in every spelling and every class body.
///
/// Called first thing after name extraction from three of the four routes
/// into a class body: [`super::attrs::lower_class_attr`] (annotated),
/// [`super::attrs::lower_unannotated_class_attr`] (#910's bare assignment),
/// and [`super::enum_class`]'s member loop. `route` distinguishes the last of
/// those, because only the `__slots__` explanation differs between them. The
/// fourth route -- a `@property` getter in [`super::body`]'s method loop --
/// calls [`reject_reserved_property_name`] instead, because only the
/// protocol-name half of this guard applies there.
///
/// Because it runs during the class-body walk, it sits at the *head* of
/// D-235's pinned four-deep diagnostic precedence rather than reordering it.
pub(super) fn reject_reserved_class_attr_name(
    attr_name: &str,
    route: ClassBodyRoute,
    range: std::ops::Range<u32>,
) -> Result<(), Diagnostic> {
    if attr_name == "__slots__" {
        return Err(unsupported(slots_message(route), range));
    }
    if let Some(message) = instantiation_protocol_message(attr_name) {
        return Err(unsupported(message, range));
    }
    Ok(())
}

/// Rejects a `@property` getter named after the instantiation or
/// class-creation protocol (#975, D-236; added in the #978 review round), or
/// named `__slots__` (#980).
///
/// The fourth route into a class-level binding of one of these names, and the
/// only one that does not go through [`reject_reserved_class_attr_name`].
/// `super::body`'s method loop routes `@property def __new__(self) -> int` to
/// `MethodKind::PropertyGetter`, so none of the three attribute routes sees
/// it; without this call `ensure_init` synthesizes a constructor from the
/// method table alone and pycc accepts `C()`, while CPython 3.13.9 raises
/// `TypeError: 'property' object is not callable` at that call.
///
/// `__slots__` is rejected here too, since
/// [#980](https://github.com/rotnov/pycc/issues/980), but under
/// [`PROPERTY_SLOTS_MESSAGE`] rather than [`slots_message`], which stays
/// deliberately unreachable from this route. The mechanism is a different one:
/// `type.__new__` *iterates* `__slots__` while the `class` statement itself
/// executes, so CPython never creates the class at all (measured on CPython
/// 3.13.9: `TypeError: 'property' object is not iterable` at class creation,
/// on a plain and on a `@dataclass` body alike), whereas D-154's "the instance
/// layout is fixed from `__init__`" describes a redundant *declaration* on a
/// class that is created. Emitting the attribute-route string here would be a
/// false account, which is what #980 was opened to avoid.
///
/// The new branch takes no [`ClassBodyRoute`]. Only one class body reaches
/// this function: `super::body`'s method loop serves the plain and the
/// `@dataclass` route alike, and `super::enum_class` rejects a method
/// definition outright before any of this, so a route parameter would carry a
/// permanently dead arm.
///
/// The three *protocol* messages are shared verbatim with the attribute
/// routes and need no property-specific clause: each already says "binding
/// that name to a non-callable object", and a `property` object is exactly
/// that. `__slots__` is the one name whose message is not shared, because its
/// reason is not shared either.
///
/// Only the *getter* arm calls this, and a `@<name>.setter` is unreachable
/// for these names for two independent reasons. `super::classify_decorator`
/// requires a setter's own `def` name to equal the decorator's property name,
/// so `@value.setter def __new__` is already rejected there as a mismatch;
/// and the matching spelling `@__new__.setter def __new__` requires a
/// preceding `@property def __new__` getter, which is rejected here first.
///
/// `@staticmethod def __new__` and `@classmethod def __init_subclass__` are
/// deliberately *not* routed here: those bind a callable, which is what
/// CPython itself expects of the protocol, so they are not the divergence
/// this guard describes.
pub(super) fn reject_reserved_property_name(
    prop_name: &str,
    range: std::ops::Range<u32>,
) -> Result<(), Diagnostic> {
    if prop_name == "__slots__" {
        return Err(unsupported(PROPERTY_SLOTS_MESSAGE, range));
    }
    match instantiation_protocol_message(prop_name) {
        Some(message) => Err(unsupported(message, range)),
        None => Ok(()),
    }
}

/// The `C0001` message for a `@property` getter named `__slots__` (#980).
///
/// The *third* account this name carries, and the only one that is not a
/// [`ClassBodyRoute`] arm of [`slots_message`] -- a constant rather than a
/// match, because the single route that reaches it cannot branch.
///
/// It describes the mechanism CPython actually fails by: `type.__new__`
/// iterates `__slots__` while the `class` statement itself executes, and a
/// `property` object is not iterable, so the class is never created. It
/// carries none of D-154's "instance layout is fixed at compile time from its
/// `__init__`" language, which is the plain attribute route's reason and would
/// be a false account here; keeping the two strings disjoint is also what lets
/// the tests pin the distinction from both sides.
///
/// Unlike [`instantiation_protocol_message`]'s strings this one **names the
/// bound type**, `property`. The distinction is worth stating once: those omit
/// the type because the guard runs before value extraction and the type is
/// whatever an initializer happens to evaluate to, while here the carrier type
/// is fixed structurally by the `@property` decorator, whatever the getter
/// returns. A message may name a type when it is structurally fixed, never
/// when it would have to be derived from an initializer.
const PROPERTY_SLOTS_MESSAGE: &str = "a `@property` getter named `__slots__` is not supported yet -- `type.__new__` iterates \
     `__slots__` while the `class` statement itself executes, and a `property` object is not \
     iterable, so CPython never creates the class at all (measured on CPython 3.13.9: \
     `TypeError: 'property' object is not iterable` at class creation), while this compiler \
     lowers the getter as an ordinary property and creates the class anyway, so the program \
     would compile and run here instead of failing";

/// Which class-body route reached the guard.
///
/// Only the `__slots__` message depends on it (see [`slots_message`]); the
/// instantiation-protocol names share one string across all routes, because
/// the reason they are reserved -- pycc resolves each protocol without
/// consulting a class attribute of that name -- is the same everywhere.
///
/// It covers the three *attribute* routes only. The `@property` getter route
/// does not pass a `ClassBodyRoute` at all: it is a single arm serving both a
/// plain and a `@dataclass` body, so it carries its own constant
/// ([`PROPERTY_SLOTS_MESSAGE`]) instead of a variant here.
///
/// `Plain` covers both the ordinary and the `@dataclass` class body: they
/// share `super::attrs`, and a dataclass instance layout is still fixed from
/// `__init__`, so D-154's explanation is accurate for both.
pub(super) enum ClassBodyRoute {
    /// [`super::attrs`]: an ordinary or `@dataclass` class body.
    Plain,
    /// [`super::enum_class`]'s member loop.
    Enum,
}

/// The `C0001` message for a class-body `__slots__` binding, per route (#910,
/// and the #975 review round that made the enum route reachable).
///
/// The routes need different text because they are rejected for different
/// reasons, and stating the wrong one is a false explanation rather than a
/// stylistic slip:
///
/// * A plain (or `@dataclass`) class has an instance layout, and pycc fixes it
///   at compile time from `__init__` (D-154), which is exactly what `__slots__`
///   declares -- so the declaration is redundant and would be discarded.
/// * An enum has neither. `lower_enum_class` produces no `__init__` and no
///   instance layout at all; its members are a compile-time table. The name is
///   rejected there because CPython gives it a *third* meaning again -- an
///   ordinary class attribute -- that pycc does not model.
///
/// A third `__slots__` account exists and is deliberately **not** an arm here:
/// [`PROPERTY_SLOTS_MESSAGE`], for the `@property` getter route (#980). It
/// stays outside this match because that route has no [`ClassBodyRoute`] to
/// dispatch on -- one arm of this enum could never be produced for it, and a
/// dead arm fails D-014's region gate.
fn slots_message(route: ClassBodyRoute) -> &'static str {
    match route {
        ClassBodyRoute::Plain => {
            "`__slots__` in a class body is not supported yet -- a class's instance layout is \
             fixed at compile time from its `__init__` (the `__slots__` semantics are already \
             implicit), so a `__slots__` assignment would be silently ignored rather than \
             honored"
        }
        ClassBodyRoute::Enum => {
            "`__slots__` in an `Enum` body is not supported yet -- CPython's `_EnumDict` keeps \
             a dunder out of the member list, so there `__slots__` is an ordinary class \
             attribute and the members are unaffected (measured on CPython 3.13.9: `class \
             C(Enum): __slots__ = \"x\"` alongside `A = 1` leaves `C.__slots__ == \'x\'` and \
             `list(C) == [C.A]`), while this compiler has no enum-specific model for the name \
             and would otherwise lower it as a member -- an enum produced by `lower_enum_class` \
             has no `__init__` and no instance layout for `__slots__` to declare, only a \
             compile-time member table -- so the program is rejected rather than compiled to a \
             different member list"
        }
    }
}

/// The `C0001` message for a class attribute named after the instantiation or
/// class-creation protocol, or `None` if the name is not one of them (#975).
///
/// Each name gets its own sentence rather than one template with a
/// substituted noun: "the constructor" is wrong for `__new__` and for
/// `__init_subclass__`, and "instantiation" is wrong for `__init_subclass__`.
///
/// Only `__init__` and `__new__` name CPython's `TypeError`, because the same
/// string is emitted from the `Enum` member route, where CPython does *not*
/// raise for `__init_subclass__`.
///
/// Neither of them names the *type* of the bound object in that `TypeError`.
/// CPython's own text is `'<type>' object is not callable`, and `<type>` is
/// whatever the initializer evaluates to (`'int'`, `'str'`, `'bool'`,
/// `'float'`, ...). Deriving it here is deliberately not done: this guard
/// runs on the attribute *name* alone, before any value extraction, and must
/// stay cheap and value-independent so that all four class-body routes can
/// call it at the same early point. Naming one concrete type would make the
/// message wrong for every other binding, so the type is omitted instead.
/// [`PROPERTY_SLOTS_MESSAGE`] does name a type, and the difference is the
/// rule: it may, because the `@property` decorator fixes the carrier type
/// structurally, while here the type would have to be derived from an
/// initializer this guard has not read.
///
/// Both messages name the `TypeError` *conditionally* in two axes. They say
/// "binding that name ... makes CPython raise" rather than "CPython raises here",
/// and they do not fix *when* it raises: the `C()` call site on the plain and
/// dataclass routes, but already at class creation on the `Enum` route, where
/// `enum.py`'s `__set_name__` invokes the now-non-callable member while the
/// `class` statement itself executes.
///
/// The first conditional exists because this guard runs before the
/// value-presence check, so it
/// also covers a value-less declaration (`__init__: int`, no `=`). CPython
/// creates no class `__dict__` entry at all for a bare annotation, so an
/// unconditional "CPython raises here" would be false for that input. The
/// name is still reserved, and the rejection is still correct; only the
/// claim about CPython has to be conditional.
fn instantiation_protocol_message(attr_name: &str) -> Option<&'static str> {
    match attr_name {
        "__init__" => Some(
            "a class attribute named `__init__` is not supported yet -- Python calls it \
             implicitly when the class is instantiated (`C()` runs `type.__call__` -> `__new__` \
             -> `__init__`), so binding that name to a non-callable object makes CPython raise a \
             `TypeError` when it reaches that call -- at the `C()` call site, or already at \
             class creation in an `Enum` body -- while this compiler resolves a class's \
             constructor from its methods alone and never consults a class attribute of that \
             name, so the binding would be silently ignored rather than honored",
        ),
        "__new__" => Some(
            "a class attribute named `__new__` is not supported yet -- Python calls it \
             implicitly when the class is instantiated (`C()` runs `type.__call__` -> `__new__` \
             before `__init__`), so binding that name to a non-callable object makes CPython \
             raise a `TypeError` when it reaches that call -- at the `C()` call site, or already \
             at class creation in an `Enum` body -- while this compiler does not model \
             `__new__` at all (construction is driven entirely by `__init__`), so the binding \
             would be silently ignored rather than honored",
        ),
        "__init_subclass__" => Some(
            "a class attribute named `__init_subclass__` is not supported yet -- Python calls \
             it implicitly when a subclass is created (`class B(C)`), but this compiler resolves \
             that hook by walking the MRO's function definitions and never consults a class \
             attribute of that name, so the binding would be silently ignored rather than \
             honored",
        ),
        _ => None,
    }
}
