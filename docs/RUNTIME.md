# pycc Runtime Specification

`pycc_rt` — the static library linked into every binary. The current runtime
and every future `deny`/`--pure` artifact are pure Rust with no libpython and
no platform-visible behavior differences (cross-platform is a hard
requirement — see ARCHITECTURE.md). Planned v0.7 CPython interop is a
conditional companion runtime bundled only when a source import resolves to a
CPython-backed dependency under the selected interop policy (D-128).

## Object model

- Scalars (`int` i64-path, `float`, `bool`, `None`) are unboxed and require no heap allocation. **Current state (D-075, extended by D-131):** `None` returns lower to LLVM `void`, while a `None` value crossing the user-function parameter ABI or living in a parameter, ordinary function-local, or module-global assignment slot uses a canonical `i8 0` unit carrier. MIR's/static storage's `Ty::None` remains the semantic tag, so the physically identical width does not make the value `False`; separate initialization flags guard local/global reads before their assignment executes. `int`'s fast path additionally low-bit-tags its word (D-061/D-141): odd words are ordinary smallints, exact words `2`/`6` preserve `False`/`True` identity in an int-compatible slot, and non-zero aligned `..00` words are heap bigint pointers. Zero and unrecognized `..10` words fail closed before pointer casts. Standalone `bool` remains `i8`; numeric runtime operations consume the markers as `0`/`1`, arithmetic returns ordinary ints, and formatting renders the markers as `False`/`True`. Every arithmetic/comparison/formatting operation on an int-compatible word is a `pycc_rt` function call rather than a raw LLVM instruction. Add, subtract, a product of two inline ints, and an inline floor-division quotient outside the tagged range promote to the heap bigint representation. An `int` literal, and an `enum` member's discriminant, whose magnitude falls outside the tagged range is materialized at run time by `pycc_rt_int_from_i64` (#148, D-178) rather than aborting code generation: the function returns the tagged word when the value round-trips and allocates a heap `BigIntObj` otherwise. It is called once per *evaluation* -- mirroring `pycc_rt_str_from_literal`, no per-literal cache -- so a bigint literal inside a loop allocates once per iteration. Since #146 Part 1 (D-180) a `BigIntObj` is refcounted and is released when a named storage slot or a loop-induction variable stops referring to it, so the `range`-loop exposure D-179 opened (one leaked object per iteration, linear in the trip count) is closed. Since #146 Part 2 (#625, D-181) that per-evaluation allocation is released too: an unbound `int` word's birth reference is retired at each site that consumes the word and discards it (both operands of an int `BinOp`/`Compare`, a discarded statement result, the five `truthy` conditions, `print`'s argument, an f-string interpolation, and a freshly built `range` bound), keyed on a compile-time classification of the source expression rather than on the word. Two temporary shapes still leak, and only these two: an `int` word stored into a tuple-literal element, and an operand whose release a D-173 exception edge branches past. Since #633 (D-182) the first of those covers a *borrowed* element word as well as a fresh one -- a tuple literal now retains a borrowed `int` element at ingress, and nothing releases a `Ty::Tuple` slot's fields -- so a supplier rebound inside a loop (`b = b + 1; t = (b, 1)`) leaks one object per trip; D-182 records the measured cost of that shape, flat at ~1.9 MB before the change against 26.1 MB at 500k trips and 50.2 MB at 1M trips after it. An `enum` discriminant materializes once at module init. Operations that require comparing an already-promoted bigint against another `int` (every comparison *operator*, not only comparison against a `float`; `range` loop control is no longer among them since #147/D-179, which uses its own `encoded_int_cmp` rather than `pycc_rt_int_cmp`), converting one to `float` (including mixed bigint/float arithmetic), multiplication/floor-division/modulo/power with a bigint operand, and a negative `int` exponent remain explicit accepted failure boundaries. So does any bigint-valued word reaching D-141's runtime `int` boundary -- a container value, a list index, a `str` repeat count, or a slice bound -- which `pycc_rt_int_untag_checked` reports as `pycc_rt: int boundary does not support bigint-valued values yet`; since #148 that boundary is reachable from a source-level literal, not only from an arithmetic promotion. `range` operands left that list in #148's follow-up #147 (D-179): they are normalized by `pycc_rt_range_normalize_operand` (bool markers to ordinary smallints, smallints and heap bigints unchanged) instead of decoded, and `pycc_rt_range_continue` compares all three operands through a sign-aware encoded comparison, so a bigint bound, a bigint step, and an induction variable that promotes mid-loop all drive the loop normally. A bigint-valued *zero* step still reports `pycc_rt: range() arg 3 must not be zero`, because the guard compares the step's numeric value rather than its encoded word. Float true division, floor division, modulo, and power also cross the runtime boundary: zero divisors now set a `ZeroDivisionError` exception state (#382, D-173) and return a neutral value instead of panicking, `//`/`%` share CPython's adjusted-remainder algorithm so rounding and signed-zero behavior are not delegated to naive LLVM division, and power rejects finite overflow plus domains that require Python exceptions or complex results instead of returning a silent infinity/NaN.
- Heap objects: no project-wide generic header exists. **Current state (through PR-10, D-105):** each heap object type defines its own header inline instead of sharing one common layout. The *refcounted* heap objects, `PyStrObj` and `PyIntListObj`, use just `rc: Cell<u32>` plus their own payload fields, with no `type_id`/`flags` anywhere in the actual runtime. A separate heap-allocated type, `BigIntObj` (the heap bigint an overflowing `int` promotes to, D-001/D-061), used to carry no header at all — no `rc`, no `type_id`, no `flags` — because D-058 never freed it. **Current state (#146 Part 1, D-180):** it carries `rc: Cell<u32>`, the same header shape as the refcounted objects above and for the same reason, and joins them as a fifth refcounted type. `pycc_rt_bigint_retain`/`pycc_rt_bigint_release` take D-141's *encoded word* rather than a pointer, are no-ops on smallints and the bool-identity markers and on the empty-slot word `0`, and are reached only under an inline `(word & 0b11) == 0 && word != 0` guard `pycc_codegen` emits so the D-084/D-140 throughput floor is untouched. D-180 narrows D-058's "never freed" half rather than superseding D-058, and enumerates what is still leaked: a `return` out of a loop body, an `int` parameter/local at function return (D-074's own `str` boundary), a call argument's retain at the callee boundary, `emit_enum_member_inits`'s instance-slot-0 word, module globals at module exit (a deliberate omission), a `bool` assigned into an `int`-declared instance attribute, and unbound arithmetic temporaries. That last entry is narrowed by #146 Part 2 (#625, D-181) down to two cases -- a fresh `int` stored into a tuple-literal element, and an operand skipped by a D-173 exception edge -- while D-180's other six are unchanged. D-181 also recorded a then-unfixed *use-after-free* (not a leak) at tuple ingress, tracked as [#633](https://github.com/rotnov/pycc/issues/633): a tuple field held a word it never retained, so overwriting the supplying name freed the object the field still pointed at, and the loop shape of that defect hung rather than printing a wrong value. **Current state (#633, D-182):** both directions are closed. The mirror direction (reading the field into a local and then overwriting that local) was already fixed by giving `retain_if_int_duplicate` a tuple-`Subscript` arm; the direction above is fixed by calling that same helper on each `MirExpr::TupleLiteral` element at ingress, so a tuple field holds a reference of its own. Only a *borrowed* element is retained -- an owning one already arrives holding the single reference the field will keep -- which is what keeps a future `Ty::Tuple` slot-death release under D-124 balanceable. The unmatched ingress retain is the accepted new leak class recorded above. `BigIntObj` still carries no `type_id`/`flags`, so it remains no counterexample to the "no project-wide generic header" statement above. A shared/generic header with cycle-tracking, shareable, and has-finalizer flags remains a possible future design once a real consumer of those flags exists, not something any shipped object implements today. **Current state (through PR-11a, D-121/D-124):** `PyDictObj` and `PyIntSetObj` (`crates/pycc_rt/src/lib.rs`) join `PyStrObj`/`PyIntListObj` in that same `rc: Cell<u32>`-plus-payload shape — the refcounted set is four types now, not the two the D-105 sentence above counted at the time (five since D-180 added `BigIntObj`'s own `rc`, which also retires this sentence's former claim that it was the sole header-less heap type), and none of them shares a generic/common header either.
- `str`: immutable UTF-8, `{len, hash-cache}` + bytes; small-string optimization ≤ 22 bytes inline. Codepoint indexing via lazily built offset index (amortized O(1), see D-007). **Current state (through PR-5):** every `str` value is a pointer to a refcounted heap object (small-string bytes inline in that same allocation, per D-059). Every named local slot is preallocated, so reassignment decrefs the previous value even when the first lexical assignment is inside a loop; top-level completion also decrefs the final named value. Two memory-safe accepted leaks remain until `pycc_own` (v0.5) adds real lifetime tracking: an unbound temporary is never decrefed, and a `str` parameter/local is not decrefed at function return. See D-074.
- `list[T]`: growable vec of unboxed `T` where `T` is scalar/struct — `list[int]` is literally `Vec<i64>`-shaped, SIMD-friendly. **Current state (D-105, superseding D-106 through D-141):** only `list[int]` is actually implemented, as `PyIntListObj` (`crates/pycc_rt/src/lib.rs`) — `rc: Cell<u32>` plus a `Cell<Vec<i64>>` payload of int-compatible encoded words. Values retain D-141's bool markers across append/read/pop/iteration/slicing; ingress still rejects bigint-valued elements, while indices, lengths, and slice bounds remain raw counters. `list[str]`/`list[float]`/`list[bool]`/nested `list[T]` are type-checked but rejected before codegen (`T0034`); refcounting is leak-only (no `pycc_rt_int_list_incref`/`_decref` call site exists yet, D-107); negative indices are rejected rather than treated as CPython's last-element addressing (D-108). **Current state (through PR-12 Task 9, D-118):** `xs[start:stop:step]` slicing is implemented as `pycc_rt_int_list_slice(list: *mut PyIntListObj, start: i64, stop: i64, step: i64) -> *mut PyIntListObj`, always returning a genuinely new list (leak-only, matching plain list construction). Each bound is independently optional in real source (defaulting to `0`/`len(list)`/`1`); after a non-negative `start`/`stop` and positive-`step` check (extending D-108's "no negative addressing" scope cut from indexing to slicing -- a negative bound or non-positive step traps), `start`/`stop` are clamped into `[0, len]`, matching CPython's own out-of-range-slice-bound clamping. `dict`/`set` slicing stays rejected as `T0033`; `tuple[...]` slicing is a genuine v0.2 deferral (`docs/ROADMAP.md`). **Current state (through PR-12 Task 11, D-119):** `list.pop()` removes and returns the list's own last encoded element, panicking (`"pycc_rt: pop from empty list"`) on an empty list rather than CPython's catchable `IndexError`. **Current state (through PR-12, D-117):** a list comprehension desugars to the same construction primitive its literal form already uses; no separate runtime allocation or append function exists for comprehensions.
- `dict[K, V]`: insertion-ordered swiss table (CPython 3.7+ order semantics). **Current state (through PR-11a, D-121):** only `dict[str, int]` is actually implemented, as `PyDictObj` (`crates/pycc_rt/src/lib.rs`) -- a dense insertion-ordered array with linear-scan lookup, not yet a real hash table. `d[k] = v` performs insert-or-update; missing-key reads now set a `KeyError` exception state (#382, D-173) and return a neutral value instead of panicking. `for k in d:` re-reads the current length every iteration rather than hoisting it, so growing `d` from inside the loop body is a deliberate, accepted v0.2 divergence from CPython (D-123): pycc silently iterates the newly-added key(s) too, where CPython raises `RuntimeError: dictionary changed size during iteration`. Refcounting is leak-only (D-124), matching `list[int]`. **Current state (through PR-12 Task 11, D-119):** `dict.get(key, default)` is implemented as `pycc_rt_dict_get_or_default(dict: *mut PyDictObj, key: *mut PyStrObj, default: i64) -> i64`, returning the stored value or `default` on a missing key without ever panicking -- unlike `pycc_rt_dict_get`'s own `d[key]` read. Only the two-argument form ships; CPython's zero/one-argument form (returning `None` on a missing key) is a deliberate v0.2 non-goal, since this compiler has no `Optional[int]`/`None`-union representation for a `dict[str, int]`'s value type yet. **Current state (through PR-12, D-117):** a dict comprehension (`{key: value for var in <source> if cond}`, assignment-RHS position only) desugars to the same construction primitive `DictLiteral` already uses -- `pycc_codegen`'s `MirStmt::DictCompAssign` arm calls `rt.dict_new` once, then inserts each surviving key/value pair via the same `pycc_rt_dict_set` call site plain `d[k] = v` also uses, inside a loop over `<source>`. No new `pycc_rt` allocation or insert function was added for comprehensions.
- `set[T]`: **Current state (through PR-11a, D-121/D-122):** only `set[int]` is implemented, as `PyIntSetObj` (`crates/pycc_rt/src/lib.rs`) -- structurally identical to `list[int]`'s own `PyIntListObj` except insertion dedups via a linear scan. Iteration order is this implementation's own insertion order, which is not guaranteed to match CPython's own hash-dependent set iteration order -- no conformance fixture asserts byte-for-byte agreement on set iteration output (D-123). No membership test (`in`) exists yet: it parses fine (the parser produces a valid `CmpOp::In` node like any other comparison operator), but `pycc_hir`'s lowering step rejects it with the same generic `C0001` capability diagnostic used for `is`/`is not`/chained comparisons -- there is no HIR/type-checker/codegen support for it anywhere in this compiler. **Current state (through PR-12 Task 11, D-119):** `set.add(value)` is implemented as a second, user-facing call site for the already-existing `pycc_rt_int_set_add` -- no new `pycc_rt` function -- and dedups on insert exactly like set-literal construction already does. **Current state (through PR-12, D-117):** a set comprehension (`{elt for var in <source> if cond}`, assignment-RHS position only) desugars to the same construction primitive `SetLiteral` already uses -- `pycc_codegen`'s `MirStmt::SetCompAssign` arm calls `rt.int_set_new` once, then adds each surviving element via the same `build_int_set_add` helper `SetLiteral`'s per-element construction and `.add()` both already call, inside a loop over `<source>`. No new `pycc_rt` allocation or add function was added for comprehensions.
- `tuple` typed: inline struct; classes: fixed-layout structs, fields resolved to offsets at compile time. **Current state (through PR-11b, D-115/D-116):** unlike every other container row above, a `tuple[...]` value is not a `pycc_rt` heap object at all -- it never allocates. Exactly `int`/`bool`/`float` elements (any mix, any arity ≥ 1) are accepted; `pycc_codegen` maps `Ty::Tuple` to an LLVM struct type and holds the whole tuple by value as an SSA aggregate (`Scalar::Tuple(StructValue)`), built with `insertvalue` and read with `extractvalue` -- no pointer, no `alloca`-plus-GEP, and consequently no refcounting question, since D-116 excludes `str` (the only element type this runtime refcounts) from tuple's v0.2 scope. `t[k]` is codegenned only for a literal, non-negative, in-range integer index (`T0040`); a heterogeneous tuple's element type at a non-literal index is not knowable statically. Module-global and function-local tuple storage both work (a plain `alloca`/global slot holding the struct value). String conversion of a tuple (`print(t)`, f-string interpolation) and truthiness of a tuple (`if t:`/`while t:`) both type-check -- `pycc_types` places no restriction on either context -- but panic honestly in `pycc_codegen`'s `to_str`/`truthy` respectively; unlike `list`/`dict`/`set`'s own identically-shaped panics there, which predate this whole PR-11 effort or were already in place before PR-11b started, this reachability is new as of PR-11b's own tuple-literal HIR lowering (`docs/ROADMAP.md` has the matching follow-up). Passing or returning a tuple value across a function boundary is implemented at this codegen layer (`build_call_to`, `MirStmt::Return`, `emit_assign` all accept `Scalar::Tuple` with a plain pass-through) but is not yet reachable from real, unannotated Python source, for two independent reasons: `pycc_types`' private-helper signature-inference solver has no unification-friendly representation for any container literal, a pre-existing limitation shared by `list`/`dict`/`set` (D-116 point 4's correction note); and, even if that solver gap closed, `pycc_codegen`'s own `emit_expr` has no dedicated `MirExpr::Call` result-dispatch arm for a container-typed return either -- it panics for `Ty::List`/`Ty::Dict`/`Ty::Set`/`Ty::Tuple` alike (D-116's own further correction note). `for x in t:` iteration, tuple-unpacking assignment, and a `tuple[...]` annotation syntax remain unimplemented (`docs/ROADMAP.md`).

**D-141 container-value addendum:** `dict[str, int]` values and `set[int]`
elements use the same int-compatible encoded words as `list[int]`. Dict
insert/update/read/default paths preserve bool markers. Set dedup compares
decoded numeric values (`True == 1`, `False == 0`) while retaining the first
inserted encoding. All three containers validate ingress and continue to reject
bigint-valued elements/values (D-180 and D-181 both deliberately leave that
boundary where it is: a container-held bigint reference has no owner today,
and whoever widens it must add an egress retain at every container read site
first -- D-181's "container egress is owning" classification is written to
stay correct once that happens);
their lengths and positional indices remain raw
runtime counters.

## Exceptions

**Current state (PR-22 Part 1, #382, D-173):** exception handling uses
per-thread runtime state plus explicit check-and-branch propagation,
superseding D-005's native-unwinding proposal. The state carries an `i8`
active flag and a `*mut PyExceptionObj`. It is thread-local to avoid shared
mutable state across runtime tests and future generated threads; generated
programs themselves remain single-threaded. The plain `extern "C"` ABI is
unchanged and no native unwinding is used.

Supported builtin exception types: `Exception` (tag 0, catch-all),
`ValueError` (1), `TypeError` (2), `KeyError` (3), `IndexError` (4),
`ZeroDivisionError` (5), `RuntimeError` (6). The hierarchy is flat:
every type is a direct subclass of `Exception`.

Supported syntax: `try`/`except`/`else`/`finally`, `raise ExceptionType("msg")`,
bare `raise` (re-raise), `raise ... from ...` (PEP 409 cause chaining),
`except ExceptionType as e` (named bindings), bare `except:` (catch-all).
`except*` (PEP 654 exception groups) is rejected with `C0001`.

Converted runtime failure paths include integer floor division/modulo by
zero, float true/floor division and modulo by zero, list index out of range,
and missing dictionary keys. They set the pending exception and return a
neutral carrier. Codegen checks the flag immediately after MIR operations that
can set it: call nodes that may invoke a user function, constructor calls,
converted arithmetic/container failures, and complete `try` statements. Child
expressions guard themselves,
so later operands, arguments, statements, and visible effects are skipped
without adding a runtime check after pure literals, reads, comparisons, or
ordinary arithmetic. Every user function has an exceptional exit that returns
a neutral ABI value while preserving the flag for its caller.

`finally` preserves a pending exception while its body executes, then restores
it only after normal fallthrough; a `return` or new exception in `finally`
replaces the pending outcome. Bare re-raise uses a lexical stack of
handler-local saved-exception slots, including nested handlers.

Uncaught exceptions at the top level are printed to stderr
(`ExceptionType: message`) and the process exits with code 1.

Exception objects are leak-only in this first implementation. Explicit
`raise ... from cause` records `cause`; implicit `__context__` is reserved but
not wired. **Planned (post-Part 1):** `except*` groups (PEP 654), custom
exception classes, full traceback with `.py` lines, implicit exception
context, `raise ... from None`, exception lifetime management, and deletion
of an `except ... as name` binding after the handler.

## Generators & iterators

Generators/`yield from` compile to resumable state machines (struct + resume fn) — no frames, no heap unless the generator escapes. `for` over known containers lowers to plain loops (no iterator protocol overhead when types are static).

## Allocator & startup

- mimalloc bundled on all Tier-1 targets; identical behavior everywhere.
- Native and `deny`/`--pure` startup: `main()` runs directly with no
  interpreter boot. Target: `hello` binary < 2 MB, < 5 ms cold start. A
  planned permitted interop artifact initializes its bundled CPython runtime
  only for the CPython-backed boundary (D-128).
- Native module init: top-level code of native pycc modules runs once, in
  deterministic import order, at process start (statically scheduled — a
  native-module import cycle is a compile error `E0108`). Planned
  CPython-backed modules instead use the bundled interpreter's normal import
  initialization, caching, and cycle semantics inside the locked environment;
  native `E0108` rules do not reject their dependency closure (D-128).

## Transparent CPython interop (planned v0.7; not implemented)

CPython-backed packages keep ordinary, CPython-compatible source imports:

```python
import numpy as np
```

pycc classifies each resolved import as a native pycc module or a
CPython-backed dependency. A CPython-backed import generates an interop bridge
without requiring a source rewrite to `pycc.interop`. The deployment artifact
bundles the pinned CPython 3.14 runtime, the resolved package artifacts, and
their native-library closure, so the target machine does not need a separately
installed Python or ambient `site-packages`. The exact resolver, `pycc.lock`
schema, and bundle layout must be specified during v0.7 planning before implementation;
the embedded interpreter must never search an unpinned ambient environment.

The build policy controls whether that automatic bridge is permitted:

| Policy | Planned behavior |
|---|---|
| `auto` | Default. Permit every CPython-backed import root present in the source and bundle its pinned dependency closure. |
| `allowlist` | Permit only direct CPython-backed import roots listed in `[interop].allow`; their submodules and pinned transitive closure are covered by the root. Reject another direct root with `I0402`. |
| `deny` | Reject every CPython-backed import. Native pycc modules remain available and the artifact has no CPython/libpython dependency. `--pure` is the CLI shorthand. |

- A source-level `import` is sufficient intent under `auto`; pycc does not ask
  for a redundant per-package permission.
- The compiler may retain `pycc.interop.cpython` as a low-level API for
  advanced explicit handles, but ordinary package use must not require it.
- CPython-owned values use the compiler's internal `cpython.Object` boundary.
  Package stubs provide their public types; a genuinely untyped value may not
  leak into pure pycc code (`I0401`). Standard-Python conversions and supported
  buffer protocols bridge native values without pycc-only source syntax.
- Interop calls hold the embedded interpreter's GIL internally; pycc threads
  stay GIL-free outside the boundary. Packages such as NumPy may release that
  GIL internally according to their own contracts.
- The v0.7 cost model and benchmark report distinguish zero-copy buffer
  transfers from copied scalar/container marshalling. Automatic interop
  preserves Python semantics, not a promise that every boundary crossing is
  free.

## ABI & embedding

- `pycc build --lib` emits a C-ABI static/shared library + generated header: compiled Python callable from C/Rust/Go.
- Symbol naming is stable per version. Pending exception state is currently one
  thread-local slot rather than a field on an embedded runtime context, so
  fully re-entrant same-thread embedding remains planned.
