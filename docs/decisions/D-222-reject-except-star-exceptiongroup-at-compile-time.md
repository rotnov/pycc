---
id: D-222
title: "Reject `except* ExceptionGroup:` at compile time instead of CPython's runtime `TypeError`"
status: accepted
---

## D-222: Reject `except* ExceptionGroup:` at compile time instead of CPython's runtime `TypeError`
- Status: accepted
- Context: [#795](https://github.com/rotnov/pycc/issues/795) found two PEP 654
  over-acceptance gaps left by
  [D-202](./D-202-pep-654-except-star-and-exceptiongroup.md)'s original
  `except*` landing (#542). The first is uncontroversial: a `return`, `break`,
  or `continue` directly inside an `except*` clause body is a `SyntaxError` in
  CPython (`'break', 'continue' and 'return' cannot appear in an except*
  block`) and pycc accepted it, so pycc now rejects it during HIR lowering
  with `L0001`, joining the same reused post-parse context-violation family
  D-148/D-149/D-193 already established. That needs no divergence and no
  decision entry of its own.

  The second gap does. `except* ExceptionGroup:` and `except*
  BaseExceptionGroup:` are **accepted by CPython's compiler**; CPython raises
  `TypeError: catching ExceptionGroup with except* is not allowed. Use except
  instead.` at *handler-match time*, when the clause is tested against a
  raised group. pycc cannot reproduce that today. Under
  [D-173](./D-173-check-and-branch-exception-model.md)'s check-and-branch
  model a raised exception propagates through global runtime state rather
  than an allocated instance, so at match time there is no materialized group
  value to type-test; and generated `except*` dispatch has no mechanism for
  raising a `TypeError` of its own. The choice is therefore not "match
  CPython or not" but "which honest failure mode to ship in the meantime".

- Decision: reject both names, **and every user class that derives from
  either of them**, at compile time from
  `pycc_types::exception::check_try_star_stmt` with **`C0001`** ("valid Python
  this compiler does not implement yet"). The exact-name check is gated on the
  name being an *unshadowed* builtin so a module defining its own
  `ExceptionGroup` class keeps reaching the existing user-exception-class
  path; the subclass check scans the resolved class's MRO, because CPython's
  runtime `TypeError` fires for any class whose MRO reaches
  `BaseExceptionGroup`, not only for the two group names themselves. That
  scan needs no shadow gate of its own: it only ever sees a class carrying an
  `exception_type_tag`, and a module that defines its own class under a
  builtin exception name withholds the seeded builtin exception classes
  entirely, so no class in such a module is tagged and the handler reports
  `T0021` well before the scan runs. `C0001` is the honest
  code precisely because the program *is* valid Python that pycc declines to
  compile — this is not a claim that CPython rejects it. The diagnostic says
  so in its own message ("CPython rejects it at runtime with a `TypeError`,
  which this compiler cannot raise yet; use a plain `except` clause instead").

  This entry **narrows D-202** rather than rewriting it: D-202 stays accepted
  and its six simplifications stand unchanged; this adds a seventh, explicitly
  temporary, divergence in the same area and records it separately, per
  `AGENTS.md`'s rule against editing an accepted decision in place.
  [#903](https://github.com/rotnov/pycc/issues/903) tracks delivering the
  real runtime `TypeError`, at which point this rejection and its three
  diagnostic fixtures are removed.

- Alternatives:
  - **Keep accepting both names (status quo).** Rejected: pycc would silently
    compile a program CPython refuses to run, and the conformance breadth
    manifest would keep carrying an unbounded `core` gap with no diagnostic
    pinning any behaviour at all. Silent over-acceptance is the worst of the
    three outcomes: nothing tells the user their program diverges.
  - **Emit the rejection as `L0001`**, joining the `except*`
    `return`/`break`/`continue` family the same issue adds. Rejected: `L0001`
    asserts "CPython classifies this as a `SyntaxError` too", which is exactly
    what is *not* true here. Reusing it would make the code family dishonest
    and would mislead anyone reading the diagnostic into thinking CPython
    rejects the program at compile time.
  - **Implement the runtime `TypeError` now.** Rejected as out of scope for
    #795, which is a conformance-gap fix, not an exception-representation
    redesign: it requires a materialized group value at handler-match time,
    which is a change to D-173's core propagation model and to every consumer
    of it. Deferred to #903 with its dependency written down there.
  - **Reject only `ExceptionGroup`, leaving `BaseExceptionGroup` accepted.**
    Rejected: CPython's runtime check covers both, and D-202 already treats
    `BaseExceptionGroup`'s hierarchy parent as `Exception`, so leaving one of
    the two accepted would be an arbitrary half-measure with no user benefit.
  - **Reject only the two exact names, leaving subclasses accepted.**
    Rejected for the same reason, one step further: CPython raises the
    identical `TypeError` for `class G(ExceptionGroup)` as for
    `ExceptionGroup` itself, so accepting `except* G:` would compile it into
    ordinary tag matching and produce a wrong runtime answer — precisely the
    silent over-acceptance the first alternative rejects, merely spelled with
    one more class in between.

- Consequences:
  - A program that is valid Python and would run under CPython (raising
    `TypeError` at the moment the handler is matched) now fails to compile
    under pycc. That is a real, user-visible divergence, deliberately taken;
    it is bounded by being a compile-time refusal rather than a silent
    behavioural difference, and every affected program is one CPython would
    have failed at runtime anyway.
  - The PEP 654 conformance matrix row stays `◐` with this recorded as a
    `core` gap pointing at #903 rather than #795, so closing #795 does not
    quietly launder the divergence into "proven".
  - Reversing this is cheap and expected: #903 deletes two checks and three
    diagnostic fixtures and replaces them with runtime-behaviour tests. No
    data format, public API, or on-disk artefact depends on the rejection.
  - `check_try_star_stmt` now has two more per-handler-type branches to keep
    ordered correctly: the group-name check runs before the
    builtin/user-class resolution but only for an unshadowed builtin, so a
    shadowing user class still resolves through the pre-existing path; the
    MRO scan runs after that resolution and before `reject_own_constructor`,
    so a group subclass that also declares its own `__init__` reports the
    group divergence (the reason it can never be compiled) rather than the
    constructor gap (which #703 will close).
  - The `L0001` half of #795 is not airtight: a `return` inside an `except*`
    clause body that sits under `if TYPE_CHECKING:` is erased by the
    constant-fold before lowering ever sees it, so it is still accepted. That
    is a pre-existing, general property of the fold rather than anything this
    entry introduces — the same hole swallows a `break` in a `finally` — and
    it is tracked separately by
    [#905](https://github.com/rotnov/pycc/issues/905).
