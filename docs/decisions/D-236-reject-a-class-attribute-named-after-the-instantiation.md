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
  | `@dataclass class P: x: int; __new__: ClassVar[int] = 8` | runs | `TypeError` |
  | `class C(Enum): __init__ = 1; B = 2` | runs | `TypeError` at class creation |

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
  `X: T = v`, and a bare `X = v`), as a `C0001` capability rejection under
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

- Consequences:
  - Three names are now unusable as a class attribute anywhere. This is a
    behavior change for programs that previously compiled — all of which were
    mis-compiled, so the change converts a silent wrong answer into a
    diagnostic.
  - The reserved-name guard becomes the single place where "a name the
    interpreter owns" is decided, reached from three call sites: the annotated
    and bare class-attribute paths and the `Enum` member loop. Adding a fourth
    class-body route in future means adding a fourth call to it; the module doc
    says so.
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
    runs before any value extraction so that all three class-body routes can
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
