//! Class attributes named after the instantiation/class-creation protocol
//! (#975, [D-236]).
//!
//! [D-236]: ../../../../docs/decisions/D-236-reject-a-class-attribute-named-after-the-instantiation.md
//!
//! Its own child module for the same reason as `dataclass_class_vars`
//! (AGENTS.md "Keep source files decomposable"): `tests.rs` is already ~7k
//! lines. `use super::*` reaches the parent's private helpers.
//!
//! Every name is exercised on its own, never as one combined class body: the
//! class-body walk and the enum member loop both return on the first error,
//! so a body listing all three would only ever prove the first one fires.

use super::*;

/// The three names, with the distinctive fragment of each one's message.
///
/// Only `__init__` and `__new__` may name CPython's `TypeError` --
/// `__init_subclass__` shares its string with the `Enum` route, where CPython
/// does not raise. Neither of the two names a concrete bound *type* in that
/// `TypeError`; `a_non_integer_binding_...` below is the pin for that.
const RESERVED: [(&str, &str); 3] = [
    (
        "__init__",
        "resolves a class's constructor from its methods alone",
    ),
    ("__new__", "does not model `__new__` at all"),
    (
        "__init_subclass__",
        "walking the MRO's function definitions",
    ),
];

/// #975: the `ClassVar` spelling in a plain class body.
#[test]
fn a_class_var_named_after_the_instantiation_protocol_is_rejected() {
    for (name, fragment) in RESERVED {
        assert_capability_error_message(
            &format!("from typing import ClassVar\n\n\nclass C:\n    {name}: ClassVar[int] = 8\n"),
            fragment,
        );
    }
}

/// #975: the plain-annotation spelling. The guard must not be `ClassVar`-gated
/// -- `body`'s non-dataclass branch routes every `AnnAssign` to
/// `lower_class_attr` regardless of the wrapper.
#[test]
fn an_annotated_attribute_named_after_the_instantiation_protocol_is_rejected() {
    for (name, fragment) in RESERVED {
        assert_capability_error_message(&format!("class C:\n    {name}: int = 8\n"), fragment);
    }
}

/// #975: #910's bare-assignment spelling, which reaches
/// `lower_unannotated_class_attr` instead.
#[test]
fn a_bare_assignment_named_after_the_instantiation_protocol_is_rejected() {
    for (name, fragment) in RESERVED {
        assert_capability_error_message(&format!("class C:\n    {name} = 8\n"), fragment);
    }
}

/// #975: a *value-less* declaration (`__init__: int`, no `=`). The guard runs
/// before the value-presence check, so this input gets the reserved-name
/// message rather than the generic "there is nothing to fold" one. It was
/// already rejected before this change, so only the message differs -- but it
/// is why both message texts say "binding that name ... makes CPython raise"
/// rather than "CPython raises here": a bare annotation creates no class
/// `__dict__` entry in CPython, so the unconditional claim would be false.
#[test]
fn a_value_less_declaration_named_after_the_instantiation_protocol_is_rejected() {
    for (name, fragment) in RESERVED {
        assert_capability_error_message(&format!("class C:\n    {name}: int\n"), fragment);
    }
}

/// #975 review round: the guard fires on the attribute *name* alone, so the
/// same message is emitted whatever the initializer binds. CPython's own text
/// names the bound type (`'str' object is not callable` for `__init__ = "x"`),
/// which the message therefore must not do -- naming one concrete type would
/// be wrong for every other binding. Pin both directions: the reserved-name
/// message still fires for a non-integer binding, and it never names a type.
#[test]
fn a_non_integer_binding_is_rejected_without_naming_a_concrete_type() {
    for (name, fragment) in [
        (
            "__init__",
            "resolves a class's constructor from its methods alone",
        ),
        ("__new__", "does not model `__new__` at all"),
    ] {
        for body in [
            format!("class C:\n    {name} = \"x\"\n"),
            format!(
                "from typing import ClassVar\n\n\nclass C:\n    {name}: ClassVar[str] = \"x\"\n"
            ),
            format!("class C:\n    {name} = True\n"),
            format!("class C:\n    {name} = 1.5\n"),
        ] {
            let module = pycc_parser_test_helper::parse(&body);
            let diagnostic = lower_checked(&module).unwrap_err();
            assert_eq!(diagnostic.code, "C0001");
            assert!(diagnostic.message.contains(fragment), "{body}");
            assert!(!diagnostic.message.contains("'int'"), "{body}");
            assert!(
                !diagnostic.message.contains("object is not callable"),
                "{body}"
            );
        }
    }
}

/// #975: the two names D-235's `DATACLASS_IMPLICIT_DUNDERS` omits are closed
/// on the dataclass path too, because the universal guard runs in every class
/// body. `__init__` is deliberately absent here -- it is owned by
/// `DATACLASS_IMPLICIT_DUNDERS`, whose check runs first and whose message
/// D-235 pinned.
#[test]
fn a_dataclass_class_var_named_new_or_init_subclass_is_rejected() {
    for (name, fragment) in [
        ("__new__", "does not model `__new__` at all"),
        (
            "__init_subclass__",
            "walking the MRO's function definitions",
        ),
    ] {
        assert_capability_error_message(
            &format!(
                "from typing import ClassVar\n\n\n@dataclass\nclass P:\n    x: int\n    {name}: ClassVar[int] = 8\n"
            ),
            fragment,
        );
    }
}

/// #913/D-235 regression pin: `__init__` in a `@dataclass` body must keep
/// reporting D-235's own message, not the new universal one. This is what
/// keeping the two name sets disjoint buys, and it is the single most likely
/// thing to break if a future change merges them.
#[test]
fn a_dataclass_class_var_named_init_still_reports_the_d235_message() {
    assert_capability_error_message(
        "from typing import ClassVar\n\n\n@dataclass\nclass P:\n    x: int\n    __init__: ClassVar[int] = 8\n",
        "a `@dataclass` class implicitly relies on `__init__`",
    );
}

/// #975: the `Enum` member loop is the third route into a class-level
/// binding. One enum class per name -- the loop returns on the first `?`, so
/// a single body listing all three would silently test only `__init__`.
#[test]
fn an_enum_member_named_after_the_instantiation_protocol_is_rejected() {
    for (name, fragment) in RESERVED {
        assert_capability_error_message(
            &format!("from enum import Enum\n\n\nclass C(Enum):\n    {name} = 1\n    B = 2\n"),
            fragment,
        );
    }
}

/// #975: the guard fires on the declaring class's own body, before any
/// subclass is seen. `__init_subclass__` diverges only once `class B(A)`
/// exists, but the class-body walk has no whole-program information, so the
/// rejection is reported at `A` -- pin that, so a future reader does not
/// expect a `B`-site diagnostic.
#[test]
fn an_inherited_reserved_attribute_is_rejected_on_the_declaring_class() {
    assert_capability_error(
        "from typing import ClassVar\n\n\nclass A:\n    __init_subclass__: ClassVar[int] = 8\n\n\nclass B(A):\n    pass\n",
        "walking the MRO's function definitions",
        Span { start: 43, end: 79 },
    );
}

/// #975 negative: the five names D-235 lists that are *not* in the universal
/// set stay accepted outside a dataclass, plus `__hash__`. Their safety rests
/// on every consuming rewrite being `is_dataclass`-gated; if that gating is
/// ever removed, this test is the thing that should start failing.
#[test]
fn dunders_outside_the_instantiation_protocol_stay_accepted_in_a_plain_class() {
    for name in [
        "__eq__",
        "__repr__",
        "__ne__",
        "__str__",
        "__format__",
        "__hash__",
    ] {
        let module = pycc_parser_test_helper::parse(&format!(
            "from typing import ClassVar\n\n\nclass C:\n    {name}: ClassVar[int] = 8\n"
        ));
        lower_checked(&module)
            .unwrap_or_else(|e| panic!("`{name}` must still lower in a plain class body: {e:?}"));
    }
}

/// #975 negative: an `Enum` member named after a dunder outside the set is
/// still lowered as an ordinary member. This pins the current behavior, not
/// agreement with CPython -- the #978 review round measured that CPython's
/// `_EnumDict` keeps every dunder out of the member list, so this program has
/// two members here and one there. That divergence is
/// [#979](https://github.com/rotnov/pycc/issues/979), a separate name set from
/// D-236's; this test inverts when it is fixed.
#[test]
fn an_enum_member_named_after_an_unreserved_dunder_stays_accepted() {
    let module = pycc_parser_test_helper::parse(
        "from enum import Enum\n\n\nclass C(Enum):\n    __repr__ = 1\n    B = 2\n",
    );
    lower_checked(&module).expect("`__repr__` must still lower as an enum member");
}

/// #910 regression pin: `__slots__` keeps its own message and is not absorbed
/// into the new set.
#[test]
fn the_slots_message_is_unchanged() {
    assert_capability_error_message(
        "class C:\n    __slots__ = 8\n",
        "`__slots__` in a class body is not supported yet",
    );
}

/// Review round on #978: `__slots__` in an `Enum` body is a fourth shape the
/// shared guard made reachable, and D-154's "the layout is fixed from
/// `__init__`" explanation is false there -- `lower_enum_class` produces no
/// `__init__` and no instance layout at all. Pin the route-specific message,
/// and pin that the plain-class explanation is *not* what gets rendered.
///
/// CPython 3.13.9 accepts this program (`_EnumDict` keeps a dunder out of the
/// member list, so `C.__slots__` is `'x'` and `list(C)` is `[C.A]`), so the
/// rejection is conservative -- the message says so rather than claiming a
/// measured divergence.
#[test]
fn the_enum_slots_message_describes_the_enum_route() {
    let source = "from enum import Enum\n\n\nclass C(Enum):\n    __slots__ = \"x\"\n    A = 1\n";
    assert_capability_error_message(source, "`__slots__` in an `Enum` body is not supported yet");

    let module = pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();
    assert!(
        !diagnostic
            .message
            .contains("fixed at compile time from its `__init__`"),
        "the enum route must not borrow D-154's plain-class explanation, got: {}",
        diagnostic.message
    );
}

/// The empty-tuple shape, which CPython also accepts (`E.__slots__ == ()`,
/// `list(E) == [E.A]`). Without the guard it would be rejected anyway, but for
/// the unrelated reason that `()` is not an `int`/`str` literal member value --
/// so pin that the reserved-name guard wins and the `__slots__` message is what
/// a reader sees.
#[test]
fn an_empty_slots_tuple_in_an_enum_body_reports_the_slots_message() {
    assert_capability_error_message(
        "from enum import Enum\n\n\nclass C(Enum):\n    __slots__ = ()\n    A = 1\n",
        "`__slots__` in an `Enum` body is not supported yet",
    );
}

/// #975 precedence pin: the new guard runs during the class-body walk, at the
/// *head* of D-235's pinned four-deep precedence, so it wins over
/// `reject_class_attr_collisions` when a class both defines `def __init__` and
/// binds `__init__` as an attribute. Pin it so a future reorder fails here
/// rather than silently changing which defect a two-defect program reports.
#[test]
fn the_reserved_name_guard_precedes_the_method_collision_check() {
    assert_capability_error_message(
        "from typing import ClassVar\n\n\nclass C:\n    __init__: ClassVar[int] = 8\n\n    def __init__(self) -> None:\n        pass\n",
        "resolves a class's constructor from its methods alone",
    );
}

/// #978 review round: the `@property` getter arm is the *fourth* class-body
/// route to a class-level binding of one of these names, and the only one
/// that does not pass through `reject_reserved_class_attr_name`.
/// `classify_decorator` routes `@property def __new__` to
/// `MethodKind::PropertyGetter`, so before this guard existed none of the
/// three attribute routes saw it: `ensure_init` synthesized a constructor
/// from the method table alone and pycc accepted `C()` while CPython 3.13.9
/// raised `TypeError: 'property' object is not callable` there.
///
/// The three names are not equally affected today, which is why each is
/// pinned by *message* rather than merely by rejection. Only `__new__` was a
/// D-198 false acceptance; `__init__` was already rejected as `T0021`
/// ("cannot redefine function `C.__init__` with a different signature") and
/// `__init_subclass__` as an unrelated `C0001` about the MRO hook needing to
/// be statically evaluable. Both of those described the wrong defect, so this
/// test fails if either reverts to its old diagnostic.
#[test]
fn a_property_getter_named_after_the_instantiation_protocol_is_rejected() {
    for (name, fragment) in RESERVED {
        assert_capability_error_message(
            &format!("class C:\n    @property\n    def {name}(self) -> int:\n        return 1\n"),
            fragment,
        );
    }
}

/// #978 review round, negative half: only the *property spelling* is
/// rejected on this route. A plain `def __init__` is a
/// `MethodKind::Regular` and must keep lowering -- a guard that keyed on the
/// method name instead of the method kind would reject every constructor in
/// the language.
#[test]
fn a_plain_init_method_is_not_rejected_by_the_property_route() {
    let module =
        pycc_parser_test_helper::parse("class C:\n    def __init__(self) -> None:\n        pass\n");
    lower_checked(&module).expect("a plain `def __init__` must still lower");
}

/// #978 review round, negative half: an ordinary `@property` is untouched.
/// This is the `None` arm of `reject_reserved_property_name` -- the guard
/// must not grow past the three protocol names.
#[test]
fn an_ordinary_property_getter_is_not_rejected() {
    let module = pycc_parser_test_helper::parse(
        "class C:\n    @property\n    def value(self) -> int:\n        return 7\n",
    );
    lower_checked(&module).expect("`@property def value` must still lower");
}

/// #978 review round: `@property def __slots__` is **not** routed through
/// this guard. Its divergence is real but different in mechanism -- CPython
/// 3.13.9 raises `TypeError: 'property' object is not iterable` while the
/// `class` statement itself executes, because `type.__new__` iterates
/// `__slots__` -- and the plain-route `__slots__` message explains D-154's
/// instance layout instead, which would be a false account of it. Tracked as
/// [#980](https://github.com/rotnov/pycc/issues/980). Pin the current
/// acceptance so that issue's fix has to come here and invert this test
/// deliberately, rather than the scope boundary being lost silently.
#[test]
fn a_property_getter_named_slots_is_left_to_issue_980() {
    let module = pycc_parser_test_helper::parse(
        "class C:\n    @property\n    def __slots__(self) -> int:\n        return 1\n",
    );
    lower_checked(&module)
        .expect("`@property def __slots__` is out of D-236's scope until #980 is fixed");
}
