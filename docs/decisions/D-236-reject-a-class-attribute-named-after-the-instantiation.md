---
id: D-236
title: "Reject a class attribute named after the instantiation or class-creation protocol"
status: accepted
---

## D-236: Reject a class attribute named after the instantiation or class-creation protocol

- Status: accepted
- Context:
  [D-235](D-235-reject-a-dataclass-field-that-shares-its-name-with-a.md) rejects
  a `@dataclass` body binding one of six dunder names, and states the rule that
  generates that set: *the three methods pycc synthesizes for a dataclass, plus
  every dunder CPython consults for an operation pycc rewrites through one of
  those three.*

  [#975](https://github.com/rotnov/pycc/issues/975) shows that rule is
  incomplete in a way the six-name list hides. It cannot generate `__new__` or
  `__init_subclass__`, because Python's object protocol calls those *implicitly*
  rather than pycc rewriting anything through them — and it says nothing at all
  about a plain (non-dataclass) class body, where `__init__` is equally
  hazardous.

  Measured at `28a1b194` against CPython 3.13.9, each of these compiled and ran
  under pycc while CPython raised `TypeError: 'int' object is not callable`:

  | program | pycc | CPython 3.13.9 |
  |---|---|---|
  | `class C: __init__: ClassVar[int] = 8` + `C()` | runs | `TypeError` |
  | `class C: __init__: int = 8` + `C()` | runs | `TypeError` |
  | `class C: __init__ = 8` + `C()` | runs | `TypeError` |
  | `class C: __new__: ClassVar[int] = 8` + `C()` | runs | `TypeError` |
  | `class A: __init_subclass__: ClassVar[int] = 8` + `class B(A)` | runs | `TypeError` at `class B` |
  | `@dataclass class P: x: int; __new__: ClassVar[int] = 8` + `P(1)` | runs | `TypeError` |
  | `class C(Enum): __init__ = 1; B = 2` | runs | `TypeError` at class creation |

  The review round on [#978](https://github.com/rotnov/pycc/pull/978) found a
  fourth class-body route the first three rows do not reach: a `@property`
  getter. `walk_class_body` routes `@property def __new__(self) -> int` to
  `MethodKind::PropertyGetter`, not to `lower_class_attr`, so the guard was
  never called for it. Measured at `f3eb908c` against CPython 3.13.9:

  | program | pycc at `f3eb908c` | CPython 3.13.9 |
  |---|---|---|
  | `class C: @property def __new__(self) -> int` + `C()` | runs | `TypeError: 'property' object is not callable` at `C()` |
  | `class C: @property def __init__(self) -> int` + `C()` | rejected, but as `T0021` "cannot redefine function `C.__init__` with a different signature" | `TypeError: 'int' object is not callable` at `C()` |
  | `class C: @property def __init_subclass__(self) -> int` + `class B(C)` | rejected, but as an unrelated `C0001` requiring the MRO's `__init_subclass__` hook to be statically evaluable | `TypeError: 'property' object is not callable` at `class B` |

  Only the `__new__` row is a false acceptance; the other two were already
  non-zero, for defects that are not the one present. The `__init__` row names
  `'int'` rather than `'property'` because `type.__call__` looks `__init__` up
  on the instance, which invokes the getter and then calls its `int` result.

  That is a D-198 false acceptance: pycc silently compiles a program CPython
  rejects. The cause is that pycc resolves each protocol without ever consulting
  a class attribute of that name — `ensure_init`
  (`crates/pycc_hir/src/class/init.rs`) decides whether to synthesize `__init__`
  from the method table alone, the `__init_subclass__` hook is found by walking
  the MRO's function definitions, and `__new__` is not modelled at all. The
  binding is therefore silently inert rather than honored.

- Decision:
  Supersede **D-235's stated generating rule**, and only that. D-235's six-name
  set and every one of its diagnostic messages remain in force, unamended.

  The widened rule is: *a class body may not bind a name that Python's
  instantiation or class-creation protocol calls implicitly* —
  `type.__call__` → `__new__` → `__init__` for instantiation, and
  `__init_subclass__` at class creation — **plus**, for dataclasses only, the
  synthesized-method set D-235 already lists.

  The instantiation/class-creation set is `{__init__, __new__,
  __init_subclass__}`, enforced by `reject_reserved_class_attr_name`
  (`crates/pycc_hir/src/class/reserved_names.rs`) in **every** class body —
  plain, `@dataclass` and `Enum` — in every spelling (`X: ClassVar[T] = v`,
  `X: T = v`, and a bare `X = v`), plus `reject_reserved_property_name` in the
  same module for the `@property def X` spelling, as a `C0001` capability
  rejection under
  [D-224](D-224-restrict-class-level-attributes-to-scalar.md)'s "reject what you can't
  model". The two sets stay **disjoint**: `body.rs`'s dataclass check runs
  before the class-attribute path, so `__init__` in a `@dataclass` body keeps
  reporting D-235's message while `__new__` and `__init_subclass__` fall through
  to this one.

  Two honest asymmetries, recorded rather than papered over. `__init_subclass__`
  is rejected on grounds that differ by route:

  - **In an `Enum` body it can never diverge.** An `Enum` with members cannot be
    subclassed at all, so the hook is never invoked; `class C(Enum):
    __init_subclass__ = 1; B = 2` prints `2` under both engines. The rejection
    there is conservative because divergence is structurally *impossible*.
  - **In a plain class body it diverges only if the class is actually
    subclassed**, which is whole-program information the class-body walk does
    not have when the guard fires.

  Both are rejected for uniformity of the single rule, not because a divergence
  was measured at the declaration site alone.

  The review round on [#978](https://github.com/rotnov/pycc/pull/978) added a
  third asymmetry, on the `__slots__` name this guard already owned from #910.
  Routing the `Enum` member loop through the same function made a `__slots__`
  binding in an enum body reachable for the first time, and the shared message
  explained it with D-154's "the instance layout is fixed at compile time from
  `__init__`" — false for a class `lower_enum_class` produces, which has no
  `__init__` and no instance layout at all. `reject_reserved_class_attr_name`
  now takes a `ClassBodyRoute` and only the `__slots__` arm branches on it; the
  instantiation-protocol strings stay route-independent, because their reason
  ("pycc resolves each protocol without consulting a class attribute of that
  name") is the same on every route. The enum text is conservative on the same
  footing as `__init_subclass__`: CPython 3.13.9 *accepts* both `class C(Enum):
  __slots__ = "x"` and `__slots__ = ()` alongside `A = 1` — the `class`
  statement succeeds, `C.__slots__` is the bound value, `list(C)` is `[C.A]`,
  and the member still has a `__dict__` — so no divergence was measured at the
  declaration site; pycc rejects because it has no model for a non-member
  dunder in an enum body and would otherwise lower the name as a member.

  **Trigger to revisit.** The five names D-235 lists that are *not* in this set
  (`__eq__`, `__repr__`, `__ne__`, `__str__`, `__format__`) are safe in a plain
  class body **only because** every pycc rewrite that consults them is
  `is_dataclass`-gated today: `crates/pycc_mir/src/class.rs`'s early
  `if !class_def.is_dataclass { return expr.clone(); }`, and
  `crates/pycc_mir/src/expr.rs`'s `&& class_def.is_dataclass`. Ungating any of
  them for a plain class reintroduces the divergence and brings that name into
  this set. `tests/issue_975_reserved_dunder_class_attrs.rs`'s
  `dunders_outside_the_instantiation_protocol_stay_accepted_in_a_plain_class`
  is the test that should start failing if that happens.

- Alternatives:
  - **Model the binding instead of rejecting it.** A class attribute folds to a
    constant at every read, so a class with a constant `__init__` has no
    representable constructor. Modelling it means representing "the attribute
    shadows the method, and calling the class is then an error" — machinery this
    version does not have, for a program that has no useful meaning. D-224 and
    D-235's own precedent both settle this as a rejection.
  - **Move `reject_class_attr_collisions` after the field merge**, as
    [#975](https://github.com/rotnov/pycc/issues/975) suggested. Foreclosed:
    D-235's Alternatives section explicitly rejected that move ("changes which
    diagnostic a program with two independent defects reports, for no gain"),
    and D-235's Consequences pin a four-deep diagnostic precedence a reorder
    would break. A guard at the head of that precedence is consistent with the
    pin; a reorder is not.
  - **Add `__new__`/`__init_subclass__` to `DATACLASS_IMPLICIT_DUNDERS`
    instead.** That fixes only the dataclass path, leaving the plain-class path
    — which is what #975 is actually about — untouched.
  - **Add them to `DATACLASS_IMPLICIT_DUNDERS` *as well*.** Unnecessary, and it
    would put two checks in line for the same name. Keeping the sets disjoint is
    what keeps every existing D-235 message byte-stable.
  - **Gate the guard on `ClassVar`.** Fixes one spelling of three: the
    non-dataclass branch of the class-body walk routes every annotated
    declaration to the class-attribute path regardless of the wrapper, and #910
    routes bare assignments there too.
  - **Amend D-235 in place.** `AGENTS.md` forbids silently rewriting an accepted
    decision. D-235's set and messages are correct as accepted; only its rule
    text was too narrow, so a superseding entry is the honest form.
  - **Defer the `Enum` body to a separate issue.** Considered seriously, on the
    hypothesis that the enum path is governed by a *different* rule — CPython's
    `_EnumDict` excludes every dunder from membership, which would be a second
    seam needing its own name set and evidence. Accepted as decided — one rule,
    one guard, one more call site — but the evidence originally recorded here
    for it was wrong, and is corrected by
    [#978](https://github.com/rotnov/pycc/pull/978)'s review round. The probe
    was `class C(Enum): __repr__ = 1; B = 2` printing `2` under both engines,
    which does not discriminate: `C.B.value` is `2` either way, whether or not
    `__repr__` also became a member. A discriminating probe shows the second
    seam is real. Under CPython 3.13.9 `_EnumDict` keeps `__repr__` out of the
    member list (`list(C.__members__) == ['B']`, `C.__repr__` is the plain
    `int`), while pycc lowers it as a member: `for c in C` counts two here and
    one there, and `C.__repr__.value` prints `1` here where CPython raises
    `AttributeError`. That divergence is a separate defect with its own name
    set (every dunder, not an enumeration) and is tracked as
    [#979](https://github.com/rotnov/pycc/issues/979); it does not change this
    decision's own set or guard, which remain correct for the
    instantiation-protocol names.

    **Amendment, 2026-09-07 — closed by
    [#979](https://github.com/rotnov/pycc/issues/979), and one correction.**
    The deferral above is discharged by
    [D-238](./D-238-reject-enum-body-assignments-cpython-keeps-out-of-the.md),
    which rejects the divergent shapes in an `Enum` body with `C0001`. It is a
    third, route-gated set inside the same
    `reject_reserved_class_attr_name` guard, checked *after* this decision's
    names so that every message pinned here is unchanged on the `Enum` route.

    The correction is to this bullet's own description of the deferred set.
    "Its own name set (every dunder, not an enumeration)" is not what the
    measurements found. CPython's rule is `enum._EnumDict.__setitem__`, whose
    `_is_private`, `_is_sunder` and `_is_dunder` branches each keep a name out
    of the member list, and all three diverged here — `class C(Enum): __x = 1`
    beside `B = 2` gave one member under CPython 3.13.9 and two under pycc
    while not being a dunder at all, and so did `_order_ = 'B'` beside
    `B = 'b'`. D-238's set is therefore dunder-shaped **plus name-mangled
    private plus sunder-shaped**, and `_is_private` needs two arms rather than
    one: it matches the *raw* dict key against the literal `_<ClassName>__`
    prefix, so `_C__x = 1` inside `class C(Enum)` is kept out of the member
    list too, while the same spelling inside `class D(Enum)` is an ordinary
    member. Per this project's append-only rule the sentence above is left
    standing and corrected here rather than rewritten; D-238 carries the full
    eleven-shape table and the three families its predicate deliberately
    over-rejects.

- Consequences:
  - Three names are now unusable as a class attribute anywhere. This is a
    behavior change for programs that previously compiled — all of which were
    mis-compiled, so the change converts a silent wrong answer into a
    diagnostic.
  - The reserved-name guard becomes the single place where "a name the
    interpreter owns" is decided, reached from four call sites: the annotated
    and bare class-attribute paths, the `Enum` member loop, and (from the #978
    review round) `walk_class_body`'s `@property` getter arm. Adding a fifth
    class-body route in future means adding a fifth call to it; the module doc
    says so. The property route calls `reject_reserved_property_name`, not the
    full guard, so only the protocol names are checked there: `@property def
    __slots__` is a different divergence — CPython raises `TypeError:
    'property' object is not iterable` while the `class` statement itself
    executes, because `type.__new__` iterates `__slots__` — and this guard's
    `__slots__` message explains D-154's instance layout instead, which would
    be a false account of it. That shape is
    [#980](https://github.com/rotnov/pycc/issues/980).

    **Amendment, 2026-09-06 — closed by
    [#980](https://github.com/rotnov/pycc/issues/980).** The deferral above is
    fulfilled rather than reversed: `reject_reserved_property_name` now rejects
    `__slots__` on the property route too, under a *third, distinct*
    `__slots__` string (`PROPERTY_SLOTS_MESSAGE`) that accounts for the failure
    correctly — `type.__new__` iterates `__slots__` while the `class` statement
    itself executes, and a `property` object is not iterable, so CPython 3.13.9
    never creates the class. `slots_message` and its `ClassBodyRoute` are
    untouched, and the new check takes no route parameter: a plain and a
    `@dataclass` body share the one `MethodKind::PropertyGetter` arm and
    diverge identically, while an `Enum` body rejects method definitions
    outright before reaching it, so a route arm there would be a dead match arm
    under D-014's region gate. Unlike this entry's three strings, that one
    *names* the bound type (`property`), because the decorator fixes it
    structurally rather than leaving it to an initializer the guard has not
    read. Nothing else here changes: this decision's rule, name set and guard,
    and D-235, all stand as accepted, which is why the closure is recorded as a
    dated note rather than as a superseding entry. `@property def __qualname__`
    stays deferred — it is value-typed on the attribute route
    (`__qualname__: int = 1` diverges, `__qualname__: str = "D"` agrees on both
    engines), so it needs a generating rule this value-independent guard cannot
    host — and is tracked as
    [#982](https://github.com/rotnov/pycc/issues/982), pinned by
    `a_property_getter_named_qualname_is_left_to_issue_982`.

    **Amendment, 2026-09-06 — widened by
    [#984](https://github.com/rotnov/pycc/issues/984).** The note above closed
    `__slots__` on the `@property` getter route only; every other class-body
    `def` spelling of the same name was still falsely accepted. Measured at
    `e77b4b13` against CPython 3.13.9, seven of them diverged — a plain `def`,
    `@staticmethod`, `@classmethod`, `@abstractmethod`, `@override`, and a
    plain `def` inside a `@dataclass` and inside a `Protocol` body — each
    raising `TypeError: '<carrier>' object is not iterable` at class creation
    while pycc compiled and ran the program. They are now rejected by
    `reject_reserved_method_name` (which subsumes the #980 getter dispatch
    unchanged) and, for a `Protocol` body,
    `reject_reserved_protocol_method_name`. Two facts about this decision's
    scaffolding change with it, and both are recorded rather than rewritten
    above:

    - **The guard is reached from five class-body call sites, not four.** The
      three attribute routes are unchanged; `walk_class_body`'s method loop is
      now one call serving the getter spelling and every non-getter spelling
      alike; and `lower_protocol_class`'s own method walk is a genuinely
      separate fifth, because `lower_class` returns through it *before* the
      method loop, so a `Protocol` body never reaches `walk_class_body` at all.
      A guard placed only in the method loop would have left that route open.
    - **`__slots__` now carries four message accounts**, not three: the
      plain/`@dataclass` attribute route (D-154), the `Enum` member list
      (`_EnumDict`), the `@property` getter (`PROPERTY_SLOTS_MESSAGE`), and
      every other `def` spelling (`method_slots_message`). The note above says
      "third, distinct"; that text stands as accepted and this amendment
      records the fourth rather than editing it.

    **The one genuinely new normative rule.** The note above permits a message
    to name the bound type when "the decorator fixes it structurally rather
    than leaving it to an initializer the guard has not read". A plain
    `def __slots__` has no decorator at all, so that rule does not generate the
    `function` carrier the new message names. The rule is therefore generalized
    to: *a message may name the bound type when the **binding form** fixes it
    structurally — `def`, `@staticmethod`, `@classmethod`, `@property` — and
    never when it would have to be derived from a value.* Every carrier the new
    message can name (`function`, `staticmethod`, `classmethod`) is fixed by
    the binding form alone, and `instantiation_protocol_message`'s strings
    still omit the type for exactly the unchanged reason: there it would come
    from an initializer this value-independent guard has not read. Without this
    generalization the new message would name a type under a rule that does not
    generate it, which is the objection class #980 existed to prevent.

    **What #984 deliberately does not touch.** The instantiation-protocol half
    stays gated to `MethodKind::PropertyGetter`, so a plain `def __new__` is
    still accepted and `a_plain_new_method_is_left_to_issue_981` stays green:
    closing that shape still needs the widening from "a non-callable binding"
    to "any binding pycc does not model" described below, which `__slots__`
    never required because it is not in this decision's name set at all. The
    two issues share only a physical location in `walk_class_body`'s method
    loop, so #984 was not sequenced behind
    [#981](https://github.com/rotnov/pycc/issues/981). A `@__slots__.setter`
    with no preceding getter is likewise left alone: it reaches the method loop
    before the "requires a preceding `@property` getter" rejection, pycc
    already rejected it, and CPython raises `NameError: name '__slots__' is not
    defined` while evaluating the decorator expression — so the new message's
    account would be false in every clause for it. It is short-circuited inside
    the guard and pinned by
    `a_slots_setter_without_a_getter_keeps_the_missing_getter_message`.
  - **The rule is about binding one of the names to a *non-callable* object**,
    which is the literal text of all three messages. A `def __new__` binds a
    callable — exactly what CPython's protocol expects — so it is outside this
    decision even though it also diverges: measured at `1095b427`,
    `def __new__(self) -> int: return 7` followed by `c = C(); c.f()` raises
    `AttributeError: 'int' object has no attribute 'f'` under CPython 3.13.9
    (`type.__call__` binds `__new__`'s result, which is not a `C`, so
    `__init__` never runs) while pycc's `ensure_init` synthesizes a constructor
    from the method table alone and prints `1`. Closing that needs a fourth
    message, a call site keyed on `MethodKind::Regular` that still leaves the
    modelled `def __init__` alone, and a decision on the implicitly-static
    `@staticmethod def __new__` twin and the `@classmethod def
    __init_subclass__` one — i.e. a widening of this decision's generating rule
    from "a non-callable binding" to "any binding pycc does not model", not a
    fifth call to the same guard. It is tracked as
    [#981](https://github.com/rotnov/pycc/issues/981) and pinned by
    `a_plain_new_method_is_left_to_issue_981`. For the same reason
    `@staticmethod def __new__` and `@classmethod def __init_subclass__` are
    deliberately untouched by the property route's call.
  - The diagnostic precedence D-235 pinned is unchanged and now explicitly
    tested. The guard is a class-body-walk error, which is already the head of
    that four-deep order, so a program with both a reserved-name binding and a
    method collision reports the reserved-name message.
  - Each name carries its own message rather than one template with a
    substituted noun: "the constructor" is wrong for `__new__` and
    `__init_subclass__`, and "instantiation" is wrong for `__init_subclass__`.
    Only `__init__` and `__new__` name CPython's `TypeError`, because the same
    string is emitted from the `Enum` route, where CPython does not raise for
    `__init_subclass__`. Neither names the bound object's *type* in it:
    CPython's text is `'<type>' object is not callable`, and `<type>` is
    whatever the initializer evaluates to (`'int'`, `'str'`, `'bool'`,
    `'float'`, ...), while this guard keys on the attribute name alone and
    runs before any value extraction so that every class-body route can
    call it at the same cheap, value-independent point. Naming one concrete
    type would be wrong for every other binding, so the type is omitted rather
    than derived. Both name the error *conditionally*, on two axes. They say
    "binding that name ... makes CPython raise" rather than "CPython raises here",
    because the guard runs before the value-presence check and so also covers a
    value-less declaration (`__init__: int`, no `=`), for which CPython creates
    no class `__dict__` entry at all. And they do not fix *when* it raises: at
    the `C()` call site on the plain and dataclass routes, but already at class
    creation on the `Enum` route. The name stays reserved and the rejection
    stays correct either way; only the claims about CPython have to be
    conditional.
  - `docs/TYPE_SYSTEM.md` now has one place describing reserved class-attribute
    names for every class kind, instead of the `__slots__` note and the
    dataclass-`ClassVar` note describing overlapping rules separately.
