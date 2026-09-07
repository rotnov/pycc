---
id: D-238
title: "Reject Enum-body assignments CPython keeps out of the member list with C0001"
status: accepted
---

## D-238: Reject `Enum`-body assignments CPython keeps out of the member list with `C0001`

- Status: accepted
- Context:
  `lower_enum_class` (`crates/pycc_hir/src/class/enum_class.rs`) walks an
  `Enum` body's statements and turns every `Stmt::Assign` with an `int` or
  `str` literal value into an enum **member**. It has no other outcome: an
  enum lowered here is a compile-time member table, and the route constructs
  no class attributes at all.

  CPython does not work that way. `enum._EnumDict.__setitem__` decides, per
  name, whether an assignment becomes a member, and three of its branches say
  no: `_is_private`, `_is_sunder` (`_x_`), and `_is_dunder` (`__x__`).
  `_is_private` is the subtle one: it runs against the key `_EnumDict`
  actually receives, which for `__x` in `class C` is the mangled `_C__x`, and
  it tests that key against the literal `_<ClassName>__` prefix — so a source
  name *already spelled* `_C__x` inside `class C` matches it too, while the
  same spelling inside `class D` does not. A private or
  dunder name becomes an ordinary class attribute; a sunder name is enum
  bookkeeping, and an unrecognized one raises `ValueError` while the `class`
  statement itself executes.

  That is a D-198 false acceptance, reported as
  [#979](https://github.com/rotnov/pycc/issues/979) out of the review round on
  [#978](https://github.com/rotnov/pycc/pull/978), which measured that
  [D-236](./D-236-reject-a-class-attribute-named-after-the-instantiation.md)'s
  probe for the same claim did not discriminate. Eleven shapes were measured at
  `edc454ba` against CPython 3.13.9 — `python3` against
  `target/debug/pycc run` — and all three branches diverge (the last two rows
  were added by this change's own deep-review round, which found the first
  draft's shape-only `_is_private` port both under- and over-rejecting):

  | `Enum` body | CPython 3.13.9 | pycc at `edc454ba` | |
  |---|---|---|---|
  | `__repr__ = 1; B = 2` | 1 member; `C.__repr__` is the plain `int` `1` | 2 members; `C.__repr__.value` prints `1` | diverges (the issue) |
  | `__repr__ = "a"; B = "b"` | 1 | 2 | diverges |
  | `_ignore_ = ['X']; B = 2` | 1 | `C0001` | rejected *accidentally* — the value is not a literal |
  | `_order_ = 'B'; B = 2` | 1 | `C0001` | rejected *accidentally* — `int`/`str` value-kind mismatch |
  | `_order_ = 'B'; B = 'b'` | 1 | 2 | **diverges** — neither accidental guard fires |
  | `_foo_ = 1; B = 2` | `ValueError` at class creation | 2 | **diverges** (unrecognized sunder) |
  | `_name_ = 1; B = 2` | `ValueError` at class creation | 2 | **diverges** |
  | `__x = 1; B = 2` | 1 | 2 | **diverges — and is not a dunder** |
  | `_x = 1; B = 2` | 2 | 2 | agrees; must stay accepted |
  | `_C__x = 1; B = 2` in `class C(Enum)` | 1 | 2 | **diverges — and nothing mangled it** |
  | `_C__x = 1; B = 2` in `class D(Enum)` | 2 | 2 | agrees; must stay accepted |

  `def __str__(self): ...` in an `Enum` body was measured too: CPython runs it,
  pycc already rejects it with `C0001` through `lower_enum_class`'s
  non-`Assign` arm. No change was needed there.

  One dunder is not merely kept out of the member list. `__order__` is
  special-cased by `_EnumDict.__setitem__`, which rewrites the key to
  `_order_`, and `EnumType.__new__` then pops `_order_` out of the class dict
  entirely — measured on CPython 3.13.9, `class C(Enum): __order__ = 'B'`
  beside `B = 2` leaves one member and *no* `C.__order__` attribute at all
  (and `__order__ = 1` raises `TypeError: 'int' object is not iterable`). The
  dunder message says so rather than claiming every dunder survives as an
  ordinary class attribute.

  The issue proposed a dunder-only predicate. The rows above show why that is
  not enough: `__x` is not a dunder, `_order_ = 'B'` beside `B = 'b'` slips
  past the value-kind mismatch that accidentally rejected the same name beside
  `B = 2`, and `_C__x` is neither a dunder nor a shape any name-independent
  test can decide.

- Decision:
  Reject the whole `_EnumDict` non-member family in an `Enum` body with
  `C0001`, through the existing `unsupported()` helper — no new diagnostic code.
  The check is the third and last step of
  `reject_reserved_class_attr_name` (`crates/pycc_hir/src/class/reserved_names.rs`),
  which `lower_enum_class`'s member loop already calls with
  `ClassBodyRoute::Enum` before any value extraction, and it is gated on that
  route: a plain, `@dataclass` or `Protocol` body has no member list, so the
  same names stay ordinary class attributes there and stay accepted.

  The predicate matches the **source** spelling, in this order:

  1. two leading underscores **and** two trailing underscores → dunder-shaped;
  2. two leading underscores (anything else) → name-mangled private;
  3. longer than `_<ClassName>__`, starting with that literal prefix, and not
     ending in `__` → already spelled the way the compiler would have mangled
     a private name;
  4. more than two characters, one leading underscore, one trailing underscore
     → sunder-shaped;
  5. otherwise an ordinary member.

  Matching the source spelling is why `_is_private` needs **two** arms rather
  than one. pycc has no name-mangling pass; CPython's compiler mangles a
  class-body `__x` to `_C__x` before `_EnumDict` sees it, and `_is_private`
  then compares that raw key against the literal `_<ClassName>__` prefix. The
  source names CPython keeps out under that one branch are therefore two
  disjoint sets — `__x` (mangled on the way in, class-name-independent: arm 2)
  and a name already spelled `_C__x` in the source of `class C` (never
  mangled, matched only because the prefix happens to be literal: arm 3, the
  only class-name-*keyed* rule in the guard). Arm 3 is why
  `ClassBodyRoute::Enum` carries the enclosing class's name.

  Each shape gets its own message, because the mechanisms differ and one
  shared string would be a false account for three of the four. In the style
  D-236 fixes, each names what was measured on CPython 3.13.9; per D-236's
  #984 amendment none of them names a value-derived type, because this guard
  runs on the name alone.

  **The orderings above are load-bearing, and so is the position of the
  whole check.** `__slots__`, `__init__`, `__new__` and `__init_subclass__` —
  the names D-236 and #910 already own — are all dunder-shaped, so testing
  this predicate before theirs would silently repoint every one of their pinned
  messages on the `Enum` route. That failure would not be caught by
  `the_enum_slots_message_describes_the_enum_route`, which asserts the
  *absence* of the plain-class string and would still pass while the wrong
  message was emitted, so
  `the_d236_names_keep_their_own_messages_on_the_enum_route`
  (`crates/pycc_hir/src/tests/enum_non_member_names.rs`) pins it from the other
  side. Within the predicate, `__x__` matches the private prefix too, so the
  dunder shape is tested first; and `_C__x_` matches both arm 3 and arm 4, so
  arm 3 runs first — `_is_private` is the branch CPython actually takes for
  it.

  **The predicate is a deliberate superset of CPython's non-member set.** It
  never under-rejects — every name CPython keeps out of the member list
  matches one of the four shapes, arm 3 included — so no false acceptance
  survives in this family. It over-rejects in exactly four measured ways:

  * **Every sunder-shaped name**, including the ones
    `_EnumDict.__setitem__` allowlists rather than raising on. Read off the
    pinned CPython 3.13.9 `enum.py` source rather than from recollection
    (`inspect.getsource(enum._EnumDict.__setitem__)`), that allowlist is
    `_order_`, `_generate_next_value_`, `_numeric_repr_`, `_missing_`,
    `_ignore_`, `_iter_member_`, `_iter_member_by_value_`,
    `_iter_member_by_def_`, `_add_alias_`, `_add_value_alias_`, and any
    `_repr_`-prefixed name. None of them is a member under either engine, but
    CPython *runs* those programs, and pycc models none of the behaviors they
    request — there is no `_ignore_` filtering, no `_order_` check, no
    `_missing_` hook and no `_generate_next_value_` override in this compiler.
    `_order_` is not even purely conservative: without the guard pycc lowered
    two members where CPython leaves one.
  * **Names matching only the `__`-prefix-and-suffix shape.** CPython's
    `_is_dunder` additionally requires `len > 4`, `name[2] != '_'` and
    `name[-3] != '_'`, so any name failing one of those stays an ordinary
    member there. That is a general rule, not a finite list; measured
    instances on CPython 3.13.9 are `__`, `___`, `____` and `___x___`, all of
    which appear in `C.__members__` there and are rejected here.
  * **Names with one leading underscore and two or more trailing ones.**
    CPython's `_is_sunder` also requires `name[-2] != '_'`, and no sibling
    branch claims them either, so they are ordinary members there. Measured on
    CPython 3.13.9 in `class C(Enum)`: `_x__ = 1` and `_foo___ = 1` both appear
    in `C.__members__`, while the sunder arm here rejects both. `_C__x__` is
    the same family reached from the other side — arm 3 skips it because it
    ends in `__` (as `_is_private` does), and arm 4 then claims it.
  * **A `__x` assignment in a class whose own name begins with an
    underscore.** CPython's compiler strips the class name's leading
    underscores when mangling, while `_is_private` compares against the
    unstripped name, so the two disagree exactly there. Measured on CPython
    3.13.9: `class _C(Enum): __x = 1` beside `B = 2` gives
    `list(_C.__members__) == ['_C__x', 'B']` — two members, the first under
    its *mangled* name — and `class ___(Enum)` (no name left after stripping)
    does not mangle at all, giving `['__x', 'B']`. Arm 2 rejects all of them.
    Modelling CPython here would need a real name-mangling pass: without one
    pycc would lower the member under the source name `__x` and report
    `.name` as `"__x"` where CPython reports `"_C__x"`, trading an
    over-rejection for a fresh false acceptance. `ENUM_PRIVATE_MESSAGE`
    therefore states *both* of CPython's outcomes rather than claiming the
    name is kept out of the member list. Raised by the codex reviewer on
    PR #988 and measured before being accepted.

  `_` and `_x` and `_foo` are *not* in the set and stay ordinary members,
  agreeing with CPython, and neither is `_C__x` in any class *not* called `C`. `__x_` is rejected on the private arm, correctly:
  CPython's compiler mangles two leading underscores with at most one trailing
  one, so `_EnumDict` sees `_C__x_` and keeps it out of the member list.

  Rejecting with `C0001` "not supported yet" is forward-compatible in the
  direction that matters: the predicate may later be narrowed, or a real
  non-member representation added, without breaking a program that was
  accepted.

- Alternatives:
  - **Model the name as a class attribute on the enum, matching CPython.**
    This is the issue's own option 2, and its premise — "if the class-attribute
    machinery is reachable from the enum route" — is false at this tree.
    `lower_enum_class` never constructs a class attribute at all; it populates
    `enum_members` and nothing else. Reaching CPython's behavior means a new
    non-member representation carried through HIR, MIR and codegen, plus the
    attribute-read path for `C.__repr__`, which is a capability addition
    several orders larger than the defect.
    [D-224](./D-224-restrict-class-level-attributes-to-scalar.md) ("reject what
    you can't model") selects rejection instead, and the same-route precedent
    is exact: `__slots__` in an `Enum` body is already rejected under D-236
    even though CPython runs that program.
  - **A dunder-only predicate**, as the issue text proposes. Tested and
    refuted by measurement, not by argument: `__x = 1; B = 2` gives one member
    under CPython and two here while not being a dunder, and
    `_order_ = 'B'; B = 'b'` does the same. A guard whose rule is stated as
    "`_EnumDict` keeps certain names out of the member list" while covering one
    of that function's three branches would leave the other two as live D-198
    false acceptances.
  - **A mangling-aware second arm** — reproduce CPython's mangling
    (`'_' + class_name.lstrip('_') + attr_name`) and then apply `_is_private`
    to the result, so `__x` in `class _C(Enum)` stays an accepted member.
    Refused: pycc has no name-mangling pass anywhere, so the member would be
    lowered under its *source* name and `C.MEMBER.name` would read back as
    `"__x"` where CPython gives `"_C__x"` — a new D-198 false acceptance in
    place of a documented over-rejection. Accepting these programs correctly
    requires mangling the member name too, which is a separate capability and
    a separate decision.
  - **A shape-only `_is_private` port** — reject any `_*__*` name without
    consulting the class name. This was the first draft, and the deep-review
    round refuted it in both directions at once: it under-rejected `_C__x` in
    `class C(Enum)` (a surviving D-198 false acceptance, since a shape test of
    "two leading underscores" never sees it) and, once widened by shape alone,
    would have over-rejected the identical spelling in `class D(Enum)`, which
    is an ordinary member on CPython 3.13.9. Only the class-name-keyed literal
    prefix decides both correctly, so `ClassBodyRoute::Enum` was given the
    class name rather than the predicate given a looser shape.
  - **Port CPython's three predicates exactly, keeping the recognized sunders
    and the short `__`-shaped names accepted.** Deferred. For the sunders it
    would replace a documented over-rejection with a fresh false acceptance:
    pycc implements none of the ten behaviors those names request, so accepting
    `_ignore_ = 'X'` would compile a program whose member list differs from
    CPython's for a *new* reason. For `__`, `___`, `____` and `___x___` the
    exactness is achievable without that hazard, but those spellings are
    pathological, an over-rejection there is `C0001` rather than a wrong
    answer, and each extra clause is a region D-014 requires a test to reach.
    Narrowing is a compatible follow-up whenever a real program needs it.
  - **Inline the rule in `lower_enum_class`'s member loop**, as the issue
    suggests. Refused: D-236's Consequences names
    `reject_reserved_class_attr_name` the single place where "a name the
    interpreter owns" is decided, the member loop already calls it as its first
    act, and inlining would split one rule across two files and re-open the
    ordering hazard above.
  - **A new diagnostic code.** Unnecessary — `unsupported()` already yields
    `C0001`, which every other enum-body rejection uses, so
    `docs/DIAGNOSTICS.md` is unchanged.

- Consequences:
  - Four name *shapes* are now unusable as `Enum` members. This is a behavior
    change for programs that previously compiled; every one of them was
    mis-compiled to a different member list than CPython's, except the
    recognized sunders, which are the recorded over-rejection.
  - The reserved-name guard now carries a rule that is a **shape** rather than
    an enumeration, and the first that is route-*gated* rather than merely
    route-*worded*. `ClassBodyRoute` accordingly does two jobs: it selects the
    `__slots__` message, and it decides whether this check runs at all. It
    also carries the enclosing class's name, which arm 3 needs.
  - The check ordering inside `reject_reserved_class_attr_name` is now
    load-bearing and is pinned by a test rather than by a comment alone.
  - D-236 keeps its own set, its own messages and its own guard unchanged.
    This decision does not supersede it; it adds the third set D-236 explicitly
    declined to own, and a dated amendment on D-236 records the closure and
    corrects that bullet's "every dunder, not an enumeration" description of
    the deferred set.
  - Two tests that pinned the old acceptance are inverted rather than deleted,
    with their prose rewritten:
    `an_enum_member_named_after_an_unreserved_dunder_is_rejected` in
    `crates/pycc_hir/src/tests/reserved_dunder_class_attrs.rs` and in
    `tests/issue_975_reserved_dunder_class_attrs.rs`.
  - Narrowing the predicate later is compatible; widening it to a name that is
    a member under CPython would not be, and would need its own decision.
