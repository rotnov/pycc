---
id: D-260
title: "Render a bridged CPython exception's message lazily, once, and without propagating a failing __str__"
status: accepted
---

## D-260: Render a bridged CPython exception's message lazily, once, and without propagating a failing __str__

- Status: accepted
- Context: Under `--ext`, a CPython exception that enters compiled code is
  bridged into a pycc exception (`src/ext/pycc_ext_module.c`,
  `pycc_ext_bridge_store`), and the original is kept in the thread's bridge
  table until its frame's watermark releases it. The bridge used to compute
  the pycc exception's message eagerly as `str(exc)`. CPython never formats an
  exception it only propagates or catches, so a user `__str__` saw one extra
  call per bridged exception ([#1511](https://github.com/rotnov/pycc/issues/1511)).
  Two existing contracts constrain any fix. `pycc_rt_exception_message`, which
  `print(e)` and `f"{e}"` call, returns the exception's message *borrowed*,
  and codegen classifies that read as a duplicate reference
  (`crates/pycc_codegen/src/str_rc.rs`, #1298). The call is also infallible:
  codegen emits no exception check after it
  (`crates/pycc_codegen/src/exception_render.rs`). And `pycc_rt` cannot call
  CPython itself, because the same runtime links into `native` executables.
- Decision:
  1. The bridge allocates the pycc exception without a message. The shim
     registers a resolver (`pycc_rt_exception_set_message_resolver`), and the
     runtime calls it the first time compiled code reads the message
     (`crates/pycc_rt/src/exception/message.rs`). An exception that only
     propagates, or is caught without being rendered, never runs `__str__`.
  2. The first rendering's text is cached on the pycc exception. A second
     rendering of the same caught exception reuses it, where CPython would
     call `__str__` again.
  3. A `__str__` that raises renders the pycc class name, and its exception
     is discarded, where CPython would propagate it out of `print`/the
     f-string.
  4. An exception rendered after its original was released (the host call
     that bridged it has returned) renders the class name, because nothing
     is left to call `__str__` on.
  5. `except*` never resolves a message. The matched group it derives gets
     CPython's `''` wrapper message. The unmatched rest group stays
     message-less and renders as its sole member's message. Whether an
     exception's message is lazy is recorded when it is allocated
     (`PyExceptionObj::lazy_message`), never inferred from the message
     cache, so this holds even after a handler has rendered the exception
     and re-raised it into the `except*`.
  `docs/RUNTIME.md` (the `--ext` exception-bridge paragraph) states the
  observable behavior. Rules 2-4 are the deliberate CPython deviations this
  entry records.
- Alternatives:
  - *Eager `str(exc)` at bridge time* (the previous behavior). Rejected: user
    code observes a `__str__` call CPython never makes (#1511), on every
    propagation, including through frames that never look at the exception.
  - *Lazy and uncached, calling `__str__` on every rendering.* This would
    remove rule 2, but the accessor would have to return an owned `str`,
    which changes `pycc_rt_exception_message`'s ABI for every artifact and
    reverses the #1298 ownership classification that codegen and
    `crates/pycc_codegen/src/tests/exception_message_rc.rs` pin. Rejected for
    this change as out of proportion to a repeated-render difference. A
    later change that makes the accessor owning can drop the cache without
    revisiting rules 1 or 3-5.
  - *Propagate a raising `__str__`.* This would remove rule 3, but every
    render site (`print`, f-string interpolation) would need a fallible edge
    to the enclosing handler. That is codegen work on the MIR rendering
    nodes, not on the bridge, so it is rejected here.
  - *Resolve the message when the watermark releases the original.* This
    would remove rule 4, but it calls `__str__` for every bridged exception
    still live at release, including ones never rendered. That reintroduces
    the eager call this decision removes, so it is rejected.
- Consequences: Propagation and catching match CPython exactly. The remaining
  differences are confined to rendering: repeated renders, a failing
  `__str__`, and rendering after the host call returned. `pycc_rt` gains a
  process-wide resolver hook, which the C shim registers. A `native`
  executable never registers one, so rule 1's lookup never runs CPython code
  there. `tests/issue_1511_lazy_exception_message.rs` compares the
  propagation and catch cases against CPython. The runtime unit tests in
  `exception/message.rs` pin rules 2-5. Removing rule 2 or 3 later is a
  superseding decision.
