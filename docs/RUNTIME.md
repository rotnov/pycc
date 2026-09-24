# pycc Runtime Specification

`pycc_rt` — the static library linked into every binary. The native runtime,
and so every `deny`/`--pure` artifact, is pure Rust with no libpython and
no platform-visible behavior differences (cross-platform is a hard
requirement — see ARCHITECTURE.md). v0.7 CPython interop is a conditional
companion runtime bundled only when a source import resolves to a
CPython-backed dependency that the effective interop policy admits (D-128,
#1224); today that bundle is D-248's embedded executable for
standard-library roots. The
no-libpython guarantee is a property of the `native` executable mode; the
hosted `ext` mode (a CPython extension module loaded by an external
interpreter, `pycc build --ext`) explicitly resolves its CPython symbols from
the host and does not carry it (D-244). `pycc_rt` itself stays libpython-free
even there: it owns only the pure encode/decode/classify half of D-244 rule
2's boundary (`crates/pycc_rt/src/ext_bridge.rs`), and the `PyObject*` moves
live in the fixed C shim the driver compiles beside the generated object, so
linking `libpycc_rt.a` into a `native` executable never pulls a CPython
symbol in.

## Object model

- Scalars (`int` i64-path, `float`, `bool`, `None`) are unboxed and require no heap allocation. **Current state (D-075, extended by D-131):** `None` returns lower to LLVM `void`, while a `None` value crossing the user-function parameter ABI or living in a parameter, ordinary function-local, or module-global assignment slot uses a canonical `i8 0` unit carrier. MIR's/static storage's `Ty::None` remains the semantic tag, so the physically identical width does not make the value `False`; separate initialization flags guard local/global reads before their assignment executes. `int`'s fast path additionally low-bit-tags its word (D-061/D-141): odd words are ordinary smallints, exact words `2`/`6` preserve `False`/`True` identity in an int-compatible slot, and non-zero aligned `..00` words are heap bigint pointers. Zero and unrecognized `..10` words fail closed before pointer casts. Standalone `bool` remains `i8`; numeric runtime operations consume the markers as `0`/`1`, arithmetic returns ordinary ints, and formatting renders the markers as `False`/`True`. The one exception is `&`, `|` and `^` (#1210): over two markers they return a marker, as CPython's `True & True` is `True`, while a shift or any pair with a non-`bool` operand returns an ordinary int. Those five operations -- `pycc_rt_int_lshift`, `pycc_rt_int_rshift`, `pycc_rt_int_and`, `pycc_rt_int_or` and `pycc_rt_int_xor` (`crates/pycc_rt/src/int_bitwise.rs`) -- are fully bigint-capable in both operands and in the shift count, decide every sign and zero test by value (so a non-canonical bigint holding a small value behaves as that value), and raise through D-173: `ValueError` for a negative shift count, and `OverflowError` for a left shift too large to allocate (a recorded deviation from CPython's `MemoryError`, `docs/TYPE_SYSTEM.md` rule 8). The two shifts are `expression_can_set_exception` checkpoints; `&`, `|` and `^` never raise. Every arithmetic/comparison/formatting operation on an int-compatible word is a `pycc_rt` function call rather than a raw LLVM instruction. Add, subtract, a product of two inline ints, and an inline floor-division quotient outside the tagged range promote to the heap bigint representation. An `int` literal, and an `enum` member's discriminant, whose magnitude falls outside the tagged range is materialized at run time by `pycc_rt_int_from_i64` (#148, D-178) rather than aborting code generation: the function returns the tagged word when the value round-trips and allocates a heap `BigIntObj` otherwise. It is called once per *evaluation* -- mirroring `pycc_rt_str_from_literal`, no per-literal cache -- so a bigint literal inside a loop allocates once per iteration. Since #146 Part 1 (D-180) a `BigIntObj` is refcounted and is released when a named storage slot or a loop-induction variable stops referring to it, so the `range`-loop exposure D-179 opened (one leaked object per iteration, linear in the trip count) is closed. Since #146 Part 2 (#625, D-181) that per-evaluation allocation is released too: an unbound `int` word's birth reference is retired at each site that consumes the word and discards it (both operands of an int `BinOp`/`Compare`, a discarded statement result, the five `truthy` conditions, `print`'s argument, an f-string interpolation, a freshly built `range` bound, and -- since #1166's round-11 review -- the `ndarray(n)` producer's own length), keyed on a compile-time classification of the source expression rather than on the word. Both of D-181's residual temporary shapes -- a fresh `int` word stored into a tuple-literal element, and any operand whose release a D-173 exception edge branched past -- are closed by [#638](https://github.com/rotnov/pycc/issues/638) (D-208): `guard_statement_effects` now releases a still-pending operand before branching to the installed exception target, and this protection covers `BinOp`/`Compare` operands, `Call`/`Instantiate` arguments, a `range()` preheater's start/stop/step bounds, and (added in a later review round of the same decision) `MirExpr::TupleLiteral`'s own element-evaluation loop, so an earlier owning element survives a later sibling element's raising evaluation the same way a `BinOp`'s left operand survives its right operand's. This closes the *owning*-temporary case at all six sites. Two of those six sites -- the `TupleLiteral` element loop and `Call`/`Instantiate` argument transfer -- additionally retain a *duplicate/borrowed* source before staging it for transfer (`BinOp`, `Compare`, and the `range()` preheater never call `retain_if_int_duplicate` at all, so no duplicate-retain gap exists there); that retained reference's own exception-edge release was closed by [#834](https://github.com/rotnov/pycc/issues/834) (D-212), which corrects D-208's own overstated "all six sites" framing. Since #633 (D-182) the first of those covers a *borrowed* element word as well as a fresh one -- a tuple literal now retains a borrowed `int` element at ingress, and nothing releases a `Ty::Tuple` slot's fields -- so a supplier rebound inside a loop (`b = b + 1; t = (b, 1)`) leaks one object per trip; D-182 records the measured cost of that shape, flat at ~1.9 MB before the change against 26.1 MB at 500k trips and 50.2 MB at 1M trips after it. An `enum` discriminant materializes once at module init. Operations that require comparing an already-promoted bigint against another `int` (every comparison *operator*, not only comparison against a `float`; `range` loop control is no longer among them since #147/D-179, which uses its own `encoded_int_cmp` rather than `pycc_rt_int_cmp`), converting one to `float` (including mixed bigint/float arithmetic), multiplication/floor-division/modulo/power with a bigint operand, and a negative `int` exponent remain explicit accepted failure boundaries; since Part C of #1038 ([#1065](https://github.com/rotnov/pycc/issues/1065)) every one of the bigint-operand boundaries in that list fails by raising a catchable `OverflowError` (D-173) and returning a type-valid sentinel rather than by aborting the process, and the negative-exponent one raises `RuntimeError` (Part A, #1063). D-141's own runtime `int` boundary is deliberately *not* part of that conversion and still aborts at every position that uses it -- with one deliberate exception added by Part 2a of #1142 ([#1165](https://github.com/rotnov/pycc/issues/1165)), the `ndarray(n)` producer's own length, which is decoded by `pycc_rt_buffer_alloc_untag_len` (a D-173 `OverflowError` plus the sentinel `0`) rather than by `pycc_rt_int_untag_checked`, because `pycc_codegen` emits its exception guard between that decode and the allocator and so has somewhere to branch to, which the positions below do not: a bigint-valued word reaching a container value, a list index, a `str` repeat count, or a slice bound is reported by `pycc_rt_int_untag_checked` as `pycc_rt: int boundary does not support bigint-valued values yet`; since #148 that boundary is reachable from a source-level literal, not only from an arithmetic promotion. **Since #618 (T0051, `pycc_hir::int_boundary`)** an out-of-range `int` literal written directly at one of these 13 boundary positions (`range` operands are excluded, per D-179 below, since a bigint there is fully supported) is instead rejected by `pycc check`/`pycc build` at compile time, restoring the pre-#148 catch point for the literal case specifically; an arithmetically promoted bigint reaching the same position is unaffected and still hits the run-time `pycc_rt_int_untag_checked` abort above, unchanged. `range` operands left that list in #148's follow-up #147 (D-179): they are normalized by `pycc_rt_range_normalize_operand` (bool markers to ordinary smallints, smallints and heap bigints unchanged) instead of decoded, and `pycc_rt_range_continue` compares all three operands through a sign-aware encoded comparison, so a bigint bound, a bigint step, and an induction variable that promotes mid-loop all drive the loop normally. A bigint-valued *zero* step is still rejected -- the guard compares the step's numeric value rather than its encoded word -- and since #150 that rejection no longer panics: it sets a `ValueError` (`range() arg 3 must not be zero`, D-173's mechanism extended to `range`) and returns the ordinary loop-exhaustion sentinel, on both the inline-smallint and general bigint-capable paths. Float true division, floor division, modulo, and power also cross the runtime boundary: zero divisors now set a `ZeroDivisionError` exception state (#382, D-173) and return a neutral value instead of panicking, `//`/`%` share CPython's adjusted-remainder algorithm so rounding and signed-zero behavior are not delegated to naive LLVM division, and power rejects finite overflow plus domains that require Python exceptions or complex results instead of returning a silent infinity/NaN. Since Part A of #1038 ([#1063](https://github.com/rotnov/pycc/issues/1063)) those four power rejections are D-173 raises rather than panics: `float_pow` raises `ZeroDivisionError` for a zero base at a negative exponent (CPython's own class and sentence), `OverflowError` for a finite pair whose true result leaves `float` range (CPython's own class), and `RuntimeError` for a negative base at a non-integer power, while `int_pow` raises `RuntimeError` for a negative exponent -- the last two are deliberate deviations, since CPython returns a `complex` and a `float` respectively and raises nothing, so no conformant class exists to name. Each raising arm returns a `0.0`/`0` sentinel and returns immediately, which is what keeps the overflow check from relabelling the zero-base raise inside one call. `Pow` deliberately stays outside `expression_can_set_exception`'s fallible set, so the raise is observed at the next enclosing D-173 checkpoint rather than at the `**` itself.
- Heap objects: no project-wide generic header exists. **Current state (through PR-10, D-105):** each heap object type defines its own header inline instead of sharing one common layout. The *refcounted* heap objects, `PyStrObj` and `PyIntListObj`, use just `rc: Cell<u32>` plus their own payload fields, with no `type_id`/`flags` anywhere in the actual runtime. A separate heap-allocated type, `BigIntObj` (the heap bigint an overflowing `int` promotes to, D-001/D-061), used to carry no header at all — no `rc`, no `type_id`, no `flags` — because D-058 never freed it. **Current state (#146 Part 1, D-180):** it carries `rc: Cell<u32>`, the same header shape as the refcounted objects above and for the same reason, and joins them as a fifth refcounted type. `pycc_rt_bigint_retain`/`pycc_rt_bigint_release` take D-141's *encoded word* rather than a pointer, are no-ops on smallints and the bool-identity markers and on the empty-slot word `0`, and are reached only under an inline `(word & 0b11) == 0 && word != 0` guard `pycc_codegen` emits so the D-084/D-140 throughput floor is untouched. D-180 narrows D-058's "never freed" half rather than superseding D-058, and enumerates what is still leaked: a `return` out of a loop body, an `int` parameter/local at function return (D-074's own `str` boundary), a call argument's retain at the callee boundary, `emit_enum_member_inits`'s instance-slot-0 word, module globals at module exit (a deliberate omission), a `bool` assigned into an `int`-declared instance attribute, and unbound arithmetic temporaries. That last entry is narrowed by #146 Part 2 (#625, D-181) down to two cases -- a fresh `int` stored into a tuple-literal element, and an operand skipped by a D-173 exception edge -- while D-180's other six are unchanged. Both cases are since closed by [#638](https://github.com/rotnov/pycc/issues/638) (D-208), including the exception-edge case's `TupleLiteral`-element flavor specifically. D-181 also recorded a then-unfixed *use-after-free* (not a leak) at tuple ingress, tracked as [#633](https://github.com/rotnov/pycc/issues/633): a tuple field held a word it never retained, so overwriting the supplying name freed the object the field still pointed at, and the loop shape of that defect hung rather than printing a wrong value. **Current state (#633, D-182):** both directions are closed. The mirror direction (reading the field into a local and then overwriting that local) was already fixed by giving `retain_if_int_duplicate` a tuple-`Subscript` arm; the direction above is fixed by calling that same helper on each `MirExpr::TupleLiteral` element at ingress, so a tuple field holds a reference of its own. Only a *borrowed* element is retained -- an owning one already arrives holding the single reference the field will keep -- which is what keeps a future `Ty::Tuple` slot-death release under D-124 balanceable. The unmatched ingress retain is the accepted new leak class recorded above. `BigIntObj` still carries no `type_id`/`flags`, so it remains no counterexample to the "no project-wide generic header" statement above. A shared/generic header with cycle-tracking, shareable, and has-finalizer flags remains a possible future design once a real consumer of those flags exists, not something any shipped object implements today. **Current state (through PR-11a, D-121/D-124):** `PyDictObj` and `PyIntSetObj` (`crates/pycc_rt/src/lib.rs`) join `PyStrObj`/`PyIntListObj` in that same `rc: Cell<u32>`-plus-payload shape — the refcounted set is four types now, not the two the D-105 sentence above counted at the time (five since D-180 added `BigIntObj`'s own `rc`, which also retires this sentence's former claim that it was the sole header-less heap type), and none of them shares a generic/common header either.
- `str`: immutable UTF-8, `{len, hash-cache}` + bytes; small-string optimization ≤ 22 bytes inline. Codepoint indexing via lazily built offset index (amortized O(1), see D-007). **Current state (through PR-5):** every `str` value is a pointer to a refcounted heap object (small-string bytes inline in that same allocation, per D-059). Every named local slot is preallocated, so reassignment decrefs the previous value even when the first lexical assignment is inside a loop; top-level completion also decrefs the final named value. **Current state ([#1054](https://github.com/rotnov/pycc/issues/1054)):** one memory-safe accepted leak remains until `pycc_own` (v0.5) adds real lifetime tracking -- an unbound temporary is never decrefed. A `str` parameter or function-scoped local *is* decrefed at function return: every entry-block `str` slot is released on every exit path, including a return routed through a `finally`, an implicit fall-through `return None`, and the `exception_exit` landing pad, so a D-244 hosted `ext` call no longer leaks one `PyStrObj` per call. See D-074.
- `list[T]`: growable vec of unboxed `T` where `T` is scalar/struct — `list[int]` is literally `Vec<i64>`-shaped, SIMD-friendly. **Current state (D-105, superseding D-106 through D-141):** only `list[int]` is actually implemented, as `PyIntListObj` (`crates/pycc_rt/src/lib.rs`) — `rc: Cell<u32>` plus a `Cell<Vec<i64>>` payload of int-compatible encoded words. Values retain D-141's bool markers across append/read/pop/iteration/slicing; ingress still rejects bigint-valued elements, while indices, lengths, and slice bounds remain raw counters. `list[str]`/`list[float]`/`list[bool]`/nested `list[T]` are type-checked but rejected before codegen (`T0034`); refcounting is leak-only (no `pycc_rt_int_list_incref`/`_decref` call site exists yet, D-107); negative indices are rejected rather than treated as CPython's last-element addressing (D-108). **Current state (through PR-12 Task 9, D-118):** `xs[start:stop:step]` slicing is implemented as `pycc_rt_int_list_slice(list: *mut PyIntListObj, start: i64, stop: i64, step: i64) -> *mut PyIntListObj`, always returning a genuinely new list (leak-only, matching plain list construction). Each bound is independently optional in real source (defaulting to `0`/`len(list)`/`1`); after a non-negative `start`/`stop` and positive-`step` check (extending D-108's "no negative addressing" scope cut from indexing to slicing -- a negative bound or non-positive step raises a catchable `ValueError`, Part B of #1038, #1064), `start`/`stop` are clamped into `[0, len]`, matching CPython's own out-of-range-slice-bound clamping. `dict`/`set` slicing stays rejected as `T0033`; `tuple[...]` slicing is a genuine v0.2 deferral (`docs/ROADMAP.md`). **Current state (through PR-12 Task 11, D-119):** `list.pop()` removes and returns the list's own last encoded element; on an empty list it raises CPython's own catchable `IndexError: pop from empty list` (D-173, Part B of #1038, #1064) rather than aborting the process as it did through v0.2. **Current state (through PR-12, D-117):** a list comprehension desugars to the same construction primitive its literal form already uses; no separate runtime allocation or append function exists for comprehensions.
- `dict[K, V]`: insertion-ordered swiss table (CPython 3.7+ order semantics). **Current state (through PR-11a, D-121):** only `dict[str, int]` is actually implemented, as `PyDictObj` (`crates/pycc_rt/src/lib.rs`) -- a dense insertion-ordered array with linear-scan lookup, not yet a real hash table. `d[k] = v` performs insert-or-update; missing-key reads now set a `KeyError` exception state (#382, D-173) and return a neutral value instead of panicking. `for k in d:` re-reads the current length every iteration rather than hoisting it, so growing `d` from inside the loop body is a deliberate, accepted v0.2 divergence from CPython (D-123): pycc silently iterates the newly-added key(s) too, where CPython raises `RuntimeError: dictionary changed size during iteration`. Refcounting is leak-only (D-124), matching `list[int]`. **Current state (through PR-12 Task 11, D-119):** `dict.get(key, default)` is implemented as `pycc_rt_dict_get_or_default(dict: *mut PyDictObj, key: *mut PyStrObj, default: i64) -> i64`, returning the stored value or `default` on a missing key without ever panicking -- unlike `pycc_rt_dict_get`'s own `d[key]` read. Only the two-argument form ships; CPython's zero/one-argument form (returning `None` on a missing key) is a deliberate v0.2 non-goal, since this compiler has no `Optional[int]`/`None`-union representation for a `dict[str, int]`'s value type yet. **Current state (through PR-12, D-117):** a dict comprehension (`{key: value for var in <source> if cond}`, assignment-RHS position only) desugars to the same construction primitive `DictLiteral` already uses -- `pycc_codegen`'s `MirStmt::DictCompAssign` arm calls `rt.dict_new` once, then inserts each surviving key/value pair via the same `pycc_rt_dict_set` call site plain `d[k] = v` also uses, inside a loop over `<source>`. No new `pycc_rt` allocation or insert function was added for comprehensions.
- `set[T]`: **Current state (through PR-11a, D-121/D-122):** only `set[int]` is implemented, as `PyIntSetObj` (`crates/pycc_rt/src/lib.rs`) -- structurally identical to `list[int]`'s own `PyIntListObj` except insertion dedups via a linear scan. Iteration order is this implementation's own insertion order, which is not guaranteed to match CPython's own hash-dependent set iteration order -- no conformance fixture asserts byte-for-byte agreement on set iteration output (D-123). No membership test (`in`) exists yet: it parses fine (the parser produces a valid `CmpOp::In` node like any other comparison operator), but `pycc_hir`'s lowering step rejects it with the same generic `C0001` capability diagnostic used for general object-identity `is`/`is not` (only a comparison against a literal `None` is admitted, D-197; chained comparisons are admitted per link, see `docs/TYPE_SYSTEM.md`'s "Chained comparisons") -- there is no HIR/type-checker/codegen support for it anywhere in this compiler. **Current state (through PR-12 Task 11, D-119):** `set.add(value)` is implemented as a second, user-facing call site for the already-existing `pycc_rt_int_set_add` -- no new `pycc_rt` function -- and dedups on insert exactly like set-literal construction already does. **Current state (through PR-12, D-117):** a set comprehension (`{elt for var in <source> if cond}`, assignment-RHS position only) desugars to the same construction primitive `SetLiteral` already uses -- `pycc_codegen`'s `MirStmt::SetCompAssign` arm calls `rt.int_set_new` once, then adds each surviving element via the same `build_int_set_add` helper `SetLiteral`'s per-element construction and `.add()` both already call, inside a loop over `<source>`. No new `pycc_rt` allocation or add function was added for comprehensions.
- `tuple` typed: inline struct; classes: fixed-layout structs, fields resolved to offsets at compile time. **Current state (through PR-11b, D-115/D-116):** unlike every other container row above, a `tuple[...]` value is not a `pycc_rt` heap object at all -- it never allocates. Exactly `int`/`bool`/`float` elements (any mix, any arity ≥ 1) are accepted; `pycc_codegen` maps `Ty::Tuple` to an LLVM struct type and holds the whole tuple by value as an SSA aggregate (`Scalar::Tuple(StructValue)`), built with `insertvalue` and read with `extractvalue` -- no pointer, no `alloca`-plus-GEP, and consequently no refcounting question, since D-116 excludes `str` (the only element type this runtime refcounts) from tuple's v0.2 scope. `t[k]` is codegenned only for a literal, non-negative, in-range integer index (`T0040`); a heterogeneous tuple's element type at a non-literal index is not knowable statically. Module-global and function-local tuple storage both work (a plain `alloca`/global slot holding the struct value). String conversion of a tuple (`print(t)`, f-string interpolation) and truthiness of a tuple (`if t:`/`while t:`) both type-check -- `pycc_types` places no restriction on either context -- but panic honestly in `pycc_codegen`'s `to_str`/`truthy` respectively; unlike `list`/`dict`/`set`'s own identically-shaped panics there, which predate this whole PR-11 effort or were already in place before PR-11b started, this reachability is new as of PR-11b's own tuple-literal HIR lowering (`docs/ROADMAP.md` has the matching follow-up). Passing or returning a tuple value across a function boundary is implemented at this codegen layer (`build_call_to`, `MirStmt::Return`, `emit_assign` all accept `Scalar::Tuple` with a plain pass-through), and **since #918 Part 1 and #925 (D-228) it is reachable from real Python source** through a written `tuple[A, B, ...]` annotation, which those two parts added to the parameter/return grammar and gave the codegen call-result arm D-116 recorded as missing -- both of that note's reasons are retired, and `pycc build PATH --ext` now carries the same shape across the CPython boundary too ([#1050](https://github.com/rotnov/pycc/issues/1050)). What remains is the *unannotated* half: `pycc_types`' private-helper signature-inference solver still has no unification-friendly representation for any container literal, a limitation shared by `list`/`dict`/`set`, so a private helper's tuple parameter or return cannot be inferred. `for x in t:` iteration and tuple-unpacking assignment remain unimplemented (`docs/ROADMAP.md`).

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
`ZeroDivisionError` (5), `RuntimeError` (6). That flat seven is not the whole
builtin surface any more: **Part 2 of #543 (#739, PEP 3151)** adds the real
`OSError` hierarchy, tags `7..=22` fixed by array index (not name-resolved):
`OSError` (7), `BlockingIOError` (8), `ChildProcessError` (9),
`ConnectionError` (10), `FileExistsError` (11), `FileNotFoundError` (12),
`InterruptedError` (13), `IsADirectoryError` (14), `NotADirectoryError` (15),
`PermissionError` (16), `ProcessLookupError` (17), `TimeoutError` (18),
`BrokenPipeError` (19), `ConnectionAbortedError` (20),
`ConnectionRefusedError` (21), `ConnectionResetError` (22). Unlike the flat
seven, this is a real tree: `OSError`'s other ten names are its direct
children, and `ConnectionError`'s four names (`BrokenPipeError`,
`ConnectionAbortedError`, `ConnectionRefusedError`, `ConnectionResetError`)
are its own children -- three levels deep from `Exception`. `except OSError:`
therefore also catches every one of the other fifteen; `except
ConnectionError:` catches its four children but not a sibling such as
`TimeoutError`. The 16 new classes carry their tag directly on their
`HirClassDef` (`pycc_hir::exception::builtin_exception_class_defs`) rather
than through the original seven's name-based `match`
(`pycc_mir::exception::resolve_exception_tag`) -- see the class-table
presence gates below, which now differ between the two groups.

**Part 3 of #382 (#542, PEP 654)** adds `BaseExceptionGroup` (23) and
`ExceptionGroup` (24), and **Part A of #1038 ([#1063](https://github.com/rotnov/pycc/issues/1063))**
adds `OverflowError` (25), each by the same fixed-array-index mechanism. The
append order matters: `OverflowError` was appended after the groups so every
earlier tag keeps its value, and it parents to `Exception` rather than
CPython's own `ArithmeticError`, which pycc does not model -- the same
deliberate hierarchy simplification D-202 records for `BaseExceptionGroup`.
`except Exception:` therefore catches it, and `except OverflowError:` resolves
through its fixed class-table tag.

**[#1292](https://github.com/rotnov/pycc/issues/1292) (Part 2 of #1282)**
appends `ImportError` (26) and `ModuleNotFoundError` (27) after
`OverflowError` by the same mechanism, so every earlier tag again keeps its
value. Both carry CPython's real parentage -- `ModuleNotFoundError` ->
`ImportError` -> `Exception` -- so, unlike `OverflowError`, this is **not** a
D-202-style simplification: `except ImportError:` catches a
`ModuleNotFoundError`, `except ModuleNotFoundError:` does not catch a plain
`ImportError`, and `except Exception:` catches both. Each resolves through its
fixed class-table tag, and a user `class PluginMissing(ImportError)` is
raisable and caught by `except ImportError:`. `ImportError`'s `name`/`path`
keyword arguments and attributes are not supported (a keyword argument is
`C0001`).

**User-defined exception classes (Part 2 of #541, D-189).** A user-declared
class whose MRO reaches a builtin exception class is raisable and catchable.
HIR lowering assigns it a type tag from `FIRST_USER_EXCEPTION_TYPE_TAG..=255`
in module source order and records it on `HirClassDef::exception_type_tag`;
the builtins keep the tags below that and either carry `None` (the flat seven,
resolved by name) or a fixed tag by array index (every builtin past them; the
groups are always reconstructed with that fixed tag regardless of the raised
object's dynamic subclass -- see D-202). A module declaring more than
`MAX_USER_EXCEPTION_CLASSES` (currently 228) such classes is rejected with
`C0001` -- the tag is a `u8` on `PyExceptionObj` and in every runtime entry
point that carries one.

Because each class in a user hierarchy carries a *different* tag, a handler
naming a class accepts a **set** of tags, not one: its own plus every raisable
class whose MRO reaches it, sorted ascending so the emitted IR does not depend
on the class table's hash-map iteration order. Codegen emits one
`pycc_rt_exception_type_matches` call per tag, joined by `or`. `except
Exception:` stays a single tag, because tag 0 is already the runtime's own
catch-all.

`PyExceptionObj` carries the class name (`name: *const u8`, `name_len`)
alongside the tag. Before Part 2 the runtime derived the printed name from the
tag with a `match` over the seven builtin constants, which can name no user
class; the name now travels with the object, supplied by codegen from a
private constant and by the runtime's own `raise_builtin` from a
`&'static str`.

Two shapes stay rejected. A user exception class that declares its own
`__init__` -- or inherits one from a non-synthetic ancestor -- is `C0001`: the
message string is the only payload the exception object carries, so the
class's own fields would be silently dropped. "Non-synthetic" is D-188
provenance, not authorship of the constructor body: since #912/D-225 a
user-declared ancestor with no `__init__` of its own carries a *synthesized*
implicit one, and that ancestor is still non-synthetic, so
`class Base: pass` / `class MyError(Base, Exception): pass` keeps this
rejection rather than escaping it. And `except MyError as e:` is
`C0001` rather than merely unimplemented: binding would give `e` a
`Ty::Instance`, which every consumer reads as a `PyInstanceObj`, while the
value the runtime holds is a `PyExceptionObj`. Both wait on Part 3 of #541
(#703), which materializes a real instance.

`raise <bound value>` (`e = MyError("x"); raise e`) is `T0021` for the same
reason, and the type checker's acceptance is keyed structurally on the call
shape rather than on the inferred type -- `e` and `MyError("boom")` infer the
identical `Ty::Instance("MyError")`, so a type-keyed rule could not tell them
apart and would reinterpret a `PyInstanceObj*` as a `PyExceptionObj*`.

**Class-table presence (Part 1 of #541, D-188; widened to all 23 names by
Part 2 of #543, #739; to all 25 by Part 3 of #382, #542, D-202; to all 26 by
Part A of #1038, #1063, which appended `OverflowError`; to all 28 by #1292,
which appended `ImportError`/`ModuleNotFoundError`).** HIR
lowering synthesizes a
real `HirClassDef` for each builtin exception name, seeded before any
user statement of a module that references one of them is lowered, so they
participate in the same class table user-defined classes do. `Exception` carries a synthetic
`__init__(self, message: str)`; every other builtin inherits it through its MRO.
Three consequences:

- `class MyError(ValueError):` resolves its base and linearizes an MRO
  (`MyError`, `ValueError`, `Exception`) like any other inheritance, and
  `MyError("boom")` resolves the inherited constructor.
- `isinstance`/`issubclass` and annotations naming a builtin exception class
  resolve against the class table instead of failing to find a definition.
- A *synthetic* builtin exception class is still not a value:
  `e = ValueError("x")` remains rejected with `C0001` (unchanged by Part 1 --
  the diagnostic comes from the callable-builtin check, and a second guard in
  `class::resolve_instantiation` now backs it up), because D-173 propagates a
  raised exception through global runtime state rather than through an
  allocated instance with fields. Raising remains the only way to construct
  one. For the same reason the synthetic definitions declare no attribute
  slots -- there is no storage for a `message` slot to name, so
  `except ValueError as e: print(e.args)` reports `T0044` (before Part 1 the
  same program aborted the compiler with an internal error, since no
  `HirClassDef` existed to look the attribute up in). Attribute access on the
  *bare class name* rather than on a binding -- `ValueError.args` -- likewise
  reports `T0044` now; before Part 1 the same source reported
  `T0021 name \`ValueError\` is not defined`, because the name was absent
  from the class table entirely.

Synthetic versus user-authored is decided by *provenance*, never by a
definition's shape: HIR lowering records on the `HirModule` that it seeded,
and the type checker marks a class synthetic if and only if that record says
this compiler produced it. A class is otherwise indistinguishable -- a user
`class Exception:` with a single `def __init__(self) -> None: pass` lowers to
exactly the synthetic `Exception`'s definition, and stays the user's own
class.

The synthetic `__init__` signature deliberately diverges from CPython's
`Exception(*args)`: this compiler has no variadic-argument support, and the
supported surface (`raise ValueError("msg")`) is exactly one `str` message.

Two gates decide whether a module is seeded, and both must pass.

*The module must reference a builtin exception name somewhere.* Every entry in
the class table costs per-item work in lowering and per-function class binding
in the type checker, and a module that never spells a builtin exception name cannot
observe the difference -- so it is seeded with none of them. The reference
scan uses the AST crate's generic visitor, so every position a name can be
spelled in counts: a base class, a `raise` operand, an `except` type, an
annotation, an `isinstance`/`issubclass` argument, an attribute access, a
comprehension, an f-string interpolation, a decorator, at any nesting depth.
A string forward reference (`x: "ValueError"`) does not count, because
annotation lowering does not resolve string annotations either.

*The module's own top level must bind none of the builtin exception names.* That gate is
all-or-nothing: a module whose top level binds any of them (a `class`, `def`,
`type` alias, annotated assignment, or assignment target spelling one) is
seeded with none of them, and that name keeps its ordinary user-defined
meaning. Because it withholds the whole group, a module that shadows one name
and uses a *different* one still has no class definition behind the one it
uses -- `class Exception: ...` together with
`except ValueError as e: print(e.args)` aborts the compiler with an internal
error. That is a known gap, present since the seeding was introduced, and
Part 2 of #541 did **not** close it: raisability keys on the MRO reaching a
builtin exception class, which is orthogonal to a partially shadowed
hierarchy. It is tracked independently by
[#704](https://github.com/rotnov/pycc/issues/704).
Because `ImportError` and `ModuleNotFoundError` joined the seeded set in
#1292, a module that declares its own `class ImportError(Exception)` now
withholds seeding and fails with `C0001` "class `ImportError` inherits from
unknown class `Exception`", exactly as a user `class OverflowError(Exception)`
already did.

**Absence is not shadowing -- but that statement now splits by name-set (Part
2 of #543, #739).** For the original flat seven, absence from the class table
still reads as un-shadowed, exactly its pre-Part-1 meaning: `raise`/`except`
name-resolve independent of `env.classes`
(`pycc_mir::exception::resolve_exception_tag`), so a module that never seeded
them behaves identically to one that did. For every builtin past the flat seven
this is **no longer true**. Those names have no name-based fallback --
deliberately, so `pycc_mir::exception::handler_type_tags`'s MRO-containment
scan never needs special-casing for them -- so `raise FileNotFoundError(...)`
or `except FileNotFoundError:` for a name outside the flat seven now requires
*actual class-table presence* to count as unshadowed
(`pycc_types::exception::is_unshadowed_builtin_exception`'s
`env.classes.contains_key(name)` conjunct). An occurrence of one of those
names in a module where seeding was withheld by the shadow gate above --
because the module shadows some *other* member of the same builtin group,
possibly one it never itself uses -- therefore behaves as *not recognized*
(`T0021`, or `C0001` at a `raise`-side call expression that also matches
`KNOWN_CALLABLE_BUILTINS`, since type inference reaches that fallback before
the raise-statement's own `T0021` path), not as silently unshadowed. Without
this conjunct such an occurrence would instead reach
`handler_type_tags`'s `.expect()` and abort the compiler with an internal
error -- the conjunct exists specifically to turn that crash into a clean
diagnostic. This narrows the #704 gap's *trigger shape* for the 16 new names
(a bare `except FileNotFoundError:` with no `.args` access and no binding is
now enough to surface it) without closing #704 itself, which remains about
the flat seven's own narrower `.args`-access trigger.

Supported syntax: `try`/`except`/`else`/`finally`, `raise ExceptionType("msg")`,
bare `raise` (re-raise), `raise ... from ...` (PEP 409 cause chaining),
`except ExceptionType as e` (named bindings), bare `except:` (catch-all).

**`except*` and `ExceptionGroup`/`BaseExceptionGroup` (Part 3 of #382, #542,
PEP 654, D-202).** `raise ExceptionGroup("msg", [members...])` /
`raise BaseExceptionGroup("msg", [members...])` construct a group from a
literal list of *existing* exception values (an `except ... as e:` binding,
or another expression that already evaluates to one) -- a fresh
`SomeError("msg")` constructor call as a member, or a non-literal second
argument, is `T0021`. `except* T1: ... except* T2: ...` dispatches each
member of the raised group to its first matching clause in source order via
`pycc_rt_exception_group_partition`, which repartitions the still-unmatched
remainder after every clause; any remainder left after the last clause is
re-raised. A reconstructed subgroup handed to a clause, or re-raised as the
final remainder, is always tagged and named as plain `ExceptionGroup`,
never the original raised object's dynamic subclass (D-202). A new exception
raised inside an `except*` clause's body propagates directly past the
statement's `finally`, rather than merging into the group's still-unmatched
remainder the way CPython's derived-exception-group chaining would (D-202).
A bare, typeless `except*:` is rejected at parse time (`L0001`) rather than
reaching codegen. `BaseExceptionGroup`'s hierarchy parent is treated as
`Exception` rather than modeled as a separate `BaseException`-only branch
(D-202) -- see the decision entry for the full simplification list.

Two further `except*` rejections close the over-acceptance gaps #795
recorded (see D-223, which narrows D-202):

- A `return`, `break`, or `continue` inside an `except*` clause body is
  rejected during HIR lowering with `L0001` (`'return' in an 'except*'
  block`, and likewise for the other two), matching CPython's own
  `SyntaxError: 'break', 'continue' and 'return' cannot appear in an except*
  block`. A loop entered *within* the clause body shields `break`/`continue`
  but never `return`, exactly as CPython's compiler behaves; at module scope
  a `return` reports the pre-existing `T0024` instead, again matching
  CPython's own precedence. A `return` guarded by `if TYPE_CHECKING:` used
  to be erased by the constant-fold before lowering saw it and so stayed
  accepted -- a pre-existing, general property of that fold rather than
  anything specific to `except*`. Since
  [#905](https://github.com/rotnov/pycc/issues/905) the guarded body is
  re-walked for `L0001` context violations only (and stays silent wherever
  lowering would have reported a `C0001`, so a guarded body may still
  contain unimplemented constructs). That walk is syntactic and
  statement-level, so it deliberately still accepts a module-scope `return`
  (pycc reports `T0024` there, matching CPython), a `yield` that is not the
  whole of an expression statement (`x = (yield 3)`), a nested `def`/`class`
  body, a `from __future__ import ...`, and any body whose enclosing
  statement's non-body parts do not themselves lower (`match` cases, a
  `while`/`for` with an `else`, a non-lowering test, a non-bare-name `for`
  target or unsupported iterable, an `except` handler whose type is not a
  bare name or a non-empty tuple of bare names).
- `except* ExceptionGroup:` and `except* BaseExceptionGroup:`, and any
  `except*` handler naming a user class whose MRO reaches either of them
  (`class G(ExceptionGroup): ...` then `except* G:`), are rejected at
  compile time with `C0001`. This is a **deliberate divergence**: CPython
  accepts both at compile time and raises `TypeError: catching ExceptionGroup
  with except* is not allowed. Use except instead.` when the handler is
  matched -- for a subclass exactly as for the group class itself, which is
  why the compile-time refusal covers subclasses too. pycc has no
  materialized group value at match time (D-173
  propagates a raised exception through global runtime state rather than an
  allocated instance) and no mechanism for raising a `TypeError` from inside
  generated `except*` dispatch, so the program is refused as valid-Python-not-
  implemented-yet instead. [#903](https://github.com/rotnov/pycc/issues/903)
  tracks delivering the real runtime behavior.

Converted runtime failure paths include integer floor division/modulo by
zero, float true/floor division and modulo by zero, list index out of range,
missing dictionary keys, and a zero-step `range()` (#150). They set the
pending exception and return a neutral carrier. Codegen checks the flag
immediately after MIR operations that
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
not wired. **Planned (post-Part 2):** a materialized exception instance so
`except ... as e` can bind a user exception class and so a class with its own
`__init__` can be raised (Part 3 of #541, #703), full traceback with `.py`
lines, implicit exception context, exception lifetime management, and
deletion of an `except ... as name` binding after the handler.

## Generators & iterators

Generators/`yield from` compile to resumable state machines (struct + resume fn) — no frames, no heap unless the generator escapes. `for` over known containers lowers to plain loops (no iterator protocol overhead when types are static).

## Allocator & startup

- mimalloc bundled on all Tier-1 targets; identical behavior everywhere.
- Native and `deny`/`--pure` startup: `main()` runs directly with no
  interpreter boot. Target: `hello` binary < 2 MB, < 5 ms cold start. An
  embedded executable (standard-library roots, D-248) instead starts its
  bundled interpreter at launch and runs the compiled module under it; see
  "Embedded executables" below.
- Native module init: top-level code of native pycc modules runs once, in
  deterministic import order, at process start (statically scheduled — a
  native-module import cycle is a compile error `E0108`). Embedded-mode
  CPython-backed modules instead use the bundled interpreter's normal import
  initialization, caching, and cycle semantics inside the bundled environment
  (pinned by `pycc.lock`, which the build consumes, #1242); native `E0108` rules do not reject their dependency closure
  (D-128).

## Transparent CPython interop (embedded mode implemented for standard-library roots, locked package closures with their native libraries and the interop policy; hosted `ext` mode implemented for the scalar boundary, `str` and a scalar-element `tuple`)

CPython-backed packages keep ordinary, CPython-compatible source imports:

```python
import numpy as np
```

The rest of this section describes the **embedded** interop mode (D-128), in
which the artifact is an executable that carries its own interpreter. The hosted
`ext` mode added by D-244 shares the import classification and the typed
boundary but none of the bundling, policy, or GIL-ownership rules below: an
`ext` artifact is loaded by an external CPython that owns the environment and
the GIL, and #1025/#1026 specify its contract. Part 1 of #1025 ([#1036](https://github.com/rotnov/pycc/issues/1036))
implemented that mode for the `int` boundary, and Part 1 of #1037
([#1048](https://github.com/rotnov/pycc/issues/1048)) widened it to the
remaining scalars. Part 2 of #1037
([#1049](https://github.com/rotnov/pycc/issues/1049)) added `str` in both
directions, and Part 3
([#1050](https://github.com/rotnov/pycc/issues/1050)) added a `tuple` of
`int`/`float`/`bool` in both directions: `pycc build PATH -o OUT --ext`
compiles against `Py_LIMITED_API 0x030D0000` (stable-ABI floor CPython 3.13),
exports every public module-level function whose signature that boundary can
carry as a `METH_FASTCALL` wrapper, runs the module body in a PEP 489
`Py_mod_exec` slot, refuses to initialize on a free-threaded interpreter, and
rejects any other public signature at compile time as `C0003`.

[#1143](https://github.com/rotnov/pycc/issues/1143) extends that export set
past module-level functions: a public `@staticmethod` and a public
`@classmethod` of a public class are exported too, when the class is not a
user exception class. Such a method is published as a `PyMethodDef` entry in
its own class's `PyType_FromSpec` type object -- the host calls it as
`mod.Class.method(...)`, and **no flat `mod."Class.method"` module attribute
is ever published**. That type is immutable
(`Py_TPFLAGS_IMMUTABLETYPE`) and never an acceptable base type (no
`Py_TPFLAGS_BASETYPE`); whether it can be *instantiated* is #1145's
constructibility question below. A `@classmethod` receives the type object in `self`
and discards it, passing the same null receiver every native
`Class.method(...)` call site already passes. A `@property` getter or setter,
and any method of a private class or of a user exception class, are **not**
exported and are not `C0003`: they are excluded as representation, not as a
capability gap. A public `@staticmethod` or `@classmethod` of a public class
whose signature the boundary cannot carry *is* a `C0003`, where it was
previously skipped in silence.

[#1145](https://github.com/rotnov/pycc/issues/1145) adds public **instance
methods** to that export set, and makes publication **MRO-resolved**
(namespace-resolved since
[#1146](https://github.com/rotnov/pycc/issues/1146), below). Three
separate predicates decide what the host sees, and they are deliberately not
the same predicate.

*Which classes are published.* A class gets a type object exactly when its
MRO-resolved export set is non-empty -- when it or one of its bases exports at
least one member -- and its own name is public and carries no exception type
tag. A class that declares no exportable member of its own is published on
the strength of what it inherits.

*Which methods each type object carries.* Every exported member of the class
and of its bases, resolved along the class's MRO most-derived-first. The walk
resolves the **namespace**, not the export set, and states Python's own
attribute lookup as a mechanism rather than as a list of member kinds. Two
rules, in this order. First, a name that **any `__init__` along the MRO
assigns to `self`** is answered by the instance, never by the type -- CPython
consults the instance `__dict__` ahead of the class namespace for everything
that is not a data descriptor -- so no class owns it and no callable is
published under it, wherever in the MRO that slot was assigned and wherever
the method it hides was declared. Rule one is a *static* test, deliberately
broader than CPython's own per-instance one: a compiled instance has no
`__dict__`, and a slot declared anywhere on the MRO has a fixed offset in
every subclass -- in a single-inheritance chain by construction, since
`pycc_hir`'s `flat_attr_layout` assigns slots most-base-first, and under
multiple inheritance because `validate_mro_slot_layout` (#969) rejects
every shape where that would not hold, a single base onto an
already-validated ancestor inheriting the property transitively -- whether
or not the `__init__` assigning it is the one a given construction
reaches. The two
diverge exactly where an override's `__init__` skips its base's, and the
artifact is lossy there rather than wrong: for a `Base` assigning
`self.value` and a `Derived(Base)` whose `__init__` calls no `super()` and
which declares `def value`, CPython answers the method and the artifact
publishes nothing. Second, every other name is answered by the
**first MRO entry that binds it in the class namespace**, whatever kind binds
it, and that entry alone decides the outcome. If its binding is an export,
the method is published; if it is anything else, the name is **absent** from
the published class, and the walk never falls through to a base that exports
the same name.

So a `Derived` that binds `value` as a `@property` publishes no callable
`value` at all, exactly as Python's own attribute lookup gives the derived
property rather than `Base.value`; a derived ordinary method shadows a base
`@property` in the same way; a derived `@staticmethod` shadows a base instance
method, published under its own receiver kind; a base's `value: int = 2`
shadows a *further* base's `value()` under multiple inheritance, because a
class attribute is an ordinary entry in the class object's namespace; and a
`self.value = ...` in any `__init__` on the MRO hides a `value()` declared on
any class of that MRO, including a *more* derived one, because rule one is
position-independent. A `Protocol` base's declaration-style `def f(self) ->
int: ...` is one of those bindings too -- it is a real function object in the
protocol's namespace, which CPython resolves like any other -- so for a
`class Q(P, A)` where `P` declares `f` and `A` exports a `@staticmethod f`,
`P`'s binding wins and `mod.Q` carries no `f` at all. An annotation-only
protocol attribute is not a binding and shadows nothing. `pycc_hir` accepts each of those collisions rather than
refusing it, so this walk is the only place they are seen. Rule one also
suppresses a name a `@property` would win as a data descriptor; that is the
same conservatism, and it costs at most a getter+setter property whose class
also assigns the name in `__init__`. A read-only property is not that case:
`self.<name> = ...` against one is a `T0044` before the class compiles at
all. A class whose every
resolved name is shadowed away this way carries no type object at all rather
than an empty one. An unshadowed name is inherited across all three method
kinds alike: `mod.Derived(21).value()` reaches a `Base.value` declared only on
the base, and `mod.Derived.tag()` reaches a base's `@staticmethod`. An
inherited method's compiled body addresses its own class's attribute slots,
which is safe because `pycc_hir`'s `validate_mro_slot_layout` (#969) rejects,
at HIR lowering with `C0001`, every multiple-inheritance shape whose ancestor
layout is not a name-wise prefix of the derived one -- see that function's own
documentation for why that is the condition.

*Which classes are constructible.* A published class is **constructible**
exactly when it is not abstract, not a `Protocol` and not an enum; it is not a
user or builtin exception class; its MRO-resolved `__init__` returns `None`;
and every parameter of that `__init__` after `self` is carriable by the table
below and is not a `tuple`. A constructible class's type object drops
`Py_TPFLAGS_DISALLOW_INSTANTIATION`, gains a `tp_init`, and the host writes
`mod.Class(...).method(...)`; a class that is not constructible keeps the
non-instantiable shape above, so `mod.Class()` raises `TypeError`. A class
published only for what it inherits is constructible on these same terms, so
`mod.Derived(21)` works while `mod.Base(...)` may refuse.

An instance method is exported when its **declaring** class is not abstract,
not a `Protocol`, not an enum and not an exception class, *and* some
**public, non-exception** class whose MRO contains it is constructible. Those
two conditions on the witness are the ones stated above for publication: only a
class the artifact publishes gets a type object the host can name, so only such
a witness makes the receiver the compiled body needs obtainable. A privately
named subclass is published under no name and is therefore no witness at all,
however constructible it is, and neither is a class whose own MRO resolves the
method's name to some other binding: the witness clause is *per method*, and a
class that shadows the name answers it with its own binding rather than with
the inherited body, so it makes no receiver for that method obtainable. The
declaring class itself need not be constructible. **Every instance method excluded by that predicate is
excluded as representation, never as a `C0003`.** An `@abstractmethod` is
excluded by the declaring-class half, which a constructible subclass does not
relax: an abstract stub's body returns nothing while its annotation says
otherwise. An instance method that survives the predicate *is* held to the
boundary like any other export, so an uncarriable signature there is a
`C0003`.

Two further consequences are deliberate. `tp_init` is not a `METH_FASTCALL`
entry point, so it enforces D-244 rule 7's keyword boundary itself --
`mod.Class(3, 4, extra=1)` raises `TypeError` because the generated `tp_init`
refuses a non-empty `kwds`, not because CPython refused it first. And the
instance a constructor allocates is never freed: D-107's arena model, narrowed
by D-154, gives `pycc_rt` no ownership model, so the leak a `native` program
bounds at process exit becomes linear in the host's call count.

The table below is the canonical statement of what the `ext` boundary carries
today, and of which calls D-244 rule 7 treats as conforming; `docs/CLI_SPEC.md`,
`docs/DIAGNOSTICS.md` and the `C0003` explanation cross-reference it rather than
restating it.

A default parameter value is filled at compile time, spliced into the call's
positional argument vector while the calling module is lowered (Part 2 of #884,
[#1189](https://github.com/rotnov/pycc/issues/1189)), so it never reaches the
generated wrapper and D-244 rule 7 is unchanged by it: the wrapper's arity check
counts every declared parameter, a defaulted one included, so a host call that
omits a defaulted argument raises the wrapper's arity-mismatch `TypeError`.
[#1194](https://github.com/rotnov/pycc/issues/1194) tracks widening the host
boundary to serve defaults.

| Annotation | As a parameter | As a return type |
|---|---|---|
| `int` | carried; accepts `int` and `bool` (the `docs/TYPE_SYSTEM.md` type table's subtype rule), `OverflowError` outside the inline range | carried |
| `float` | carried; accepts `float` **only** — an `int`, a `bool` or any `__float__` duck type raises `TypeError` | carried |
| `bool` | carried; accepts `bool` **only** — an `int` or any other truthy object raises `TypeError` | carried, and identity survives: `PyBool_FromLong` returns the interned singleton |
| `None` | **not carried**: `C0003`, gated on [#1047](https://github.com/rotnov/pycc/issues/1047)'s call-argument ICE | carried, as `Py_RETURN_NONE` |
| `str` | carried; accepts `str` **only** — no `__str__`, `os.PathLike` or buffer duck type. A lone surrogate raises CPython's own `UnicodeEncodeError`, propagated verbatim | carried |
| `tuple[...]` of `int`/`bool`/`float` | carried; accepts a `tuple` or a `tuple` subclass of exactly the declared arity, each element admitted by its own `int`/`float`/`bool` row above -- `str` is carried at a top-level position but not as an element. Every other object -- `list`, `str`, an iterator, a different arity -- raises `TypeError` | carried, always as an exact `tuple` |
| `tuple[...]` carrying anything else, any other container, `T \| None` | **not carried**: `C0003` | **not carried**: `C0003` |
| `object` | **not carried**: `C0003` | **not carried**: `C0003` |
| `memoryview` (also spellable `ndarray` or `NDArray`) | carried; accepts **any object exporting a conforming buffer** — a `memoryview`, a bare NumPy array, an `array.array('d')`, a third-party exporter — since [#1129](https://github.com/rotnov/pycc/issues/1129) widened the first refusal arm from `PyMemoryView_Check` to `PyObject_CheckBuffer`. That widening applies to the `memoryview` annotation as much as to the `ndarray` spelling and to the `NDArray` spelling [#1134](https://github.com/rotnov/pycc/issues/1134) added: all three lower to one type and reach one helper. Both `ndarray` and `NDArray` are resolved after a program's own classes and type aliases, so a module-level `class ndarray` or `type NDArray = ...` keeps its own meaning (D-244's #1129 review-round-3 amendment, statement (h)). The buffer must additionally be C-contiguous, one-dimensional, and of element format `'d'` (float64); the last two raise a `TypeError` naming what it saw, and a non-contiguous operand's refusal is the **exporter's own**, propagated verbatim — a strided `memoryview` raises CPython's `BufferError` and a strided NumPy array numpy's own `ValueError`, so the propagated *type* is not fixed (D-244's #1129 amendment, statement (g)). An object that exports no buffer at all — a `list`, an `int`, `None` — is what the first arm refuses now, with a pycc-authored `TypeError` naming the function, the 1-based argument index and the offending type. A NumPy array therefore reaches the admitted slot **bare**, with no `memoryview(a.reshape(-1))` wrapper, provided it is C-contiguous, one-dimensional and `float64`; that acceptance and the refusals it did not move onto (a two-dimensional array, an element format other than `'d'`, a strided array, and a non-exporter) are pinned end to end in `tests/issue_1114_numpy_oracle.rs` (Part 3 of #1027, [#1114](https://github.com/rotnov/pycc/issues/1114)) and, on the standard library alone, in `tests/issue_1129_ndarray_buffer_carrier.rs`. The wrapper owns the `Py_buffer` for the whole call and releases it on every exit, including the success path; compiled code receives a `{ ptr, len }` pair and never the buffer or the owning object. The acquisition request is `PyBUF_C_CONTIGUOUS | PyBUF_FORMAT`, plus `PyBUF_WRITABLE` for exactly those parameters whose own function body stores into them (Part 1 of [#1142](https://github.com/rotnov/pycc/issues/1142)) — so a read-only exporter is still accepted by a parameter that is only read, and is refused by a parameter that is written with the exporter's own `BufferError`, propagated verbatim. The bit is per parameter, and is computed for a constructor's parameters exactly as for an export's. In a build without `--ext` the buffer type in a *signature* -- a parameter or the return type -- is refused as `I0405`, in the canonical `memoryview` spelling whichever way it was written. Since Part 2a of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1165](https://github.com/rotnov/pycc/issues/1165)) a native build also refuses a function *body* that allocates artifact-owned storage, under the same `I0405` code but in its own words: `pycc_rt_buffer_f64_alloc` is a plain heap allocation that links into a native executable unchanged, so that program would run -- it is refused because the buffer type exists to carry data across the CPython extension-module boundary, and a native executable has no host to carry it to. That body refusal honours statement (h) on the same terms as the producer itself, the function-local arm below included, and it is reported only for a function the type check itself admits: a call that matches the spelling and the arity but whose *length* the checker refuses -- `ndarray("x")` is `T0033`, `ndarray(missing)` is `T0021` -- is not an allocation at all, so the program gets its own diagnostic, the one `--ext` and `pycc check` report for it, rather than an `I0405` whose "rebuild with `--ext`" remedy would not apply. The join is per function, so a function that allocates and checks clean is still named when a different function in the same program fails. Only a signature admits the type at all: a buffer *declaration* (`x: memoryview`, `x: ndarray` or `x: NDArray`) is a `C0001` capability gap in both modes, because nothing in #1027 produces a value to bind to the name. Part 2a of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1165](https://github.com/rotnov/pycc/issues/1165)) adds the second source of a buffer-typed binding, and the first the artifact itself owns: `a = ndarray(n)` (or `NDArray(n)`, and deliberately **not** `memoryview(n)`, which is a `TypeError` in CPython) allocates a zero-filled one-dimensional float64 buffer of `n` elements through `pycc_rt_buffer_f64_alloc`. It is admitted in exactly one position -- the whole right-hand side of an assignment, bare or annotated, to a local name inside a function body -- and refused with `C0001` everywhere else, including at module scope, which has no frame to free it from; a length that is neither an `int` nor a `bool` is `T0033` -- `bool` is an `int` by [TYPE_SYSTEM.md](./TYPE_SYSTEM.md)'s representation table (rule 4/D-086), so `ndarray(True)` requests one element -- a negative length raises `ValueError` at runtime, a bigint-valued length raises `OverflowError` and an inline length too large to reserve storage for (`ndarray(2 ** 62 - 1)`) raises `RuntimeError`, both added by #1165's round-8 review round because each had been aborting the host interpreter instead -- and the length's own birth reference is retired before the guard that reads either raise, so a host catching the `OverflowError` in a loop does not leak one `BigIntObj` per call the way the `pycc_rt_int_to_float` consumers tracked by [#1076](https://github.com/rotnov/pycc/issues/1076) still do (#1166's round-11 review) (see this document's object-model paragraph for the decoder, and `pycc_rt_buffer_f64_alloc`'s own doc comment for the reservation), and statement (h) applies to the call position exactly as to the annotation position, so a program's own `class ndarray`, `def ndarray`, module-level `ndarray = ...`, or **function-local** binding of the spelling anywhere in the calling body keeps its own meaning; a body that binds the name anywhere makes it local throughout, as in CPython, so an earlier `ndarray(n)` there is that program's `UnboundLocalError` (`T0021`). Whether a value-less annotation (`ndarray: int` with no `= ...`) is such a meaning is a scoping question rather than a boundary one, so [TYPE_SYSTEM.md](./TYPE_SYSTEM.md)'s `memoryview` row owns it rather than this one: where that row says the producer stays in place, the native-mode body refusal above is not disarmed either, and where it says the spelling becomes function-local, that refusal has no producer to report. The single admitted position is what makes the lifetime sound: the value cannot be aliased, stored or passed, so the allocating frame is provably its only owner and frees it through `pycc_rt_buffer_f64_free` on every return path, with a reassignment freeing the old view before it stores the new one (D-074). Part 2b of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1164](https://github.com/rotnov/pycc/issues/1164)) adds the single exception — a bare `return a` from a public `--ext` export, which since [#1174](https://github.com/rotnov/pycc/issues/1174) is a public module-level `def` *or* a public method of a public class, and which transfers ownership to the host-facing exporter described in the return column rather than freeing it — and performs that transfer in the frame's owned-slot epilogue rather than at the `return` statement, so the epilogue and the exporter can never both free the same storage. That is deliberately not D-107's leak-only container precedent. Assigning to a name bound to a buffer *parameter* is `C0001` for the same reason: the wrapper borrows that storage from the host for one call, so the frame must never free it. Exactly two *reads* of a name bound to a `memoryview` are implemented, and exactly one *write*. `v[i]` (Part 2 of #1027, [#1113](https://github.com/rotnov/pycc/issues/1113)) loads one float64 element through `pycc_rt_buffer_f64_get`. That helper owns the bounds check -- it compares the index against the `len` the wrapper copied out of `Py_buffer.shape[0]` and raises `IndexError` for anything outside `0..len`, negative indices included, since a negative index is refused rather than wrapped (D-108) -- so compiled code never reads outside the view, and the index itself must be an `int` (`T0021` otherwise). `len(v)` ([#1116](https://github.com/rotnov/pycc/issues/1116)) returns that same copied `len` through `pycc_rt_buffer_len`, as an `int` counted in elements and never in bytes, so it equals CPython's own `len(view)`; it performs no check and cannot raise, and together with `v[i]` it makes `for i in range(len(v)):` -- one call sweeping the whole input -- expressible. `v[i] = x` (Part 1 of [#1142](https://github.com/rotnov/pycc/issues/1142)) stores one float64 element through `pycc_rt_buffer_f64_set`, the void sibling of the load helper: same bounds check, same `IndexError` outside `0..len` with negative indices refused rather than wrapped, same `int` index rule, and the value must be a `float` (`T0021` otherwise -- D-086 grants no int-to-float widening, so `v[0] = 1` is refused). Its operands are evaluated in CPython's own assignment order -- the value first, then the subscription target, then the index -- so a raising or side-effecting index expression can never suppress a value expression CPython would already have run. An augmented store `v[i] op= x` is lowered as `v[i] = v[i] op x` with a name-or-literal index ([#1209](https://github.com/rotnov/pycc/issues/1209); the rule and its conditions are `docs/TYPE_SYSTEM.md`'s "Augmented assignment"), so the load `v[i]`, and the `IndexError` it raises past the end, comes before the value `x`, as in CPython, and the store is the same element store the `PyBUF_WRITABLE` walk sees. Neither helper assumes the exporter's storage is aligned: none of the four properties the wrapper checks (writable, one-dimensional, C-contiguous, format `'d'`) implies 8-byte alignment, and `memoryview(bytearray(17))[1:].cast('d')` passes all four at a data address that is `1 mod 8`, so both the load and the store use unaligned accesses and such an exporter keeps working rather than becoming a refusal. Every *other* read is still that same `C0001` gap wherever it appears -- an alias, a `for` over the name itself, a call of the name, an attribute, or handing it to another function; `return a` from a public `--ext` export is the one read Part 2b of #1142 (#1164) removes from that list for artifact-owned storage, and `return b` the one Part 1 of [#1175](https://github.com/rotnov/pycc/issues/1175) (#1178) removes for a parameter-bound name -- in both cases the bare name only -- and that holds inside the body of an `--ext` export whose parameter the signature admits: the parameter position exists so the wrapper can unpack the view for the compiled boundary, not so compiled code can operate on it as a value. A `Protocol` method's signature splits the two positions: a `memoryview` parameter is still `I0405` in a native build (a class genuinely can satisfy it under `--ext`, where an exported function receives the view and passes it inward), while a `-> memoryview` member is refused at its declaration as `C0001` in *both* modes, because no class could implement it in either | carried, from one shape only, since Part 2b of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1164](https://github.com/rotnov/pycc/issues/1164)), widened to every export by [#1174](https://github.com/rotnov/pycc/issues/1174): a **public export** -- a module-level `def`, or a public `@staticmethod`, `@classmethod` or reachable instance method of a public non-exception class -- may return a bare local name holding artifact-owned storage, provided that name is definitely assigned at the `return` -- `docs/TYPE_SYSTEM.md`'s definite-assignment clause owns that condition, and a buffer allocated on only one path reaching the `return` is its ordinary `T0041` rather than an egress -- and provided the function contains no `return` statement lexically inside any `finally` body, at any nesting depth, which is `C0001` (review round 5 of [#1164](https://github.com/rotnov/pycc/issues/1164); [#1173](https://github.com/rotnov/pycc/issues/1173) tracks admitting it). The host receives a real `memoryview` — format `'d'`, `itemsize` 8, `ndim` 1, C-contiguous, **writable** — over a file-static, refcounted, limited-API heap exporter the artifact creates in its `Py_mod_exec` slot; its `tp_dealloc` performs the one `pycc_rt_buffer_f64_free`, so the storage outlives the call for exactly as long as the host holds a reference. Ownership transfers in the compiled frame's owned-slot epilogue — the one point control actually leaves the frame — and **not** at the `return` statement, which only makes the value pending (and, when it supersedes an earlier pending one, releases the predecessor the frame still solely owns -- a release, never a transfer). User finalizers run between the two, so the returned name stays bound to live storage throughout them: a `finally` that reads it sees the same buffer CPython would, and a nested `finally`, a `return` inside a loop inside a `try`, and a plain `return` with no `try` all converge on that one epilogue. The epilogue releases every owned buffer slot except the one whose pointer is the value actually being returned, so a buffer live on a path that did *not* return is still freed there — and so is a pending return that a raising finalizer abandoned, since nothing then reaches the host. A finalizer that *rebinds* the returned name cannot release it either: the frame records the pending pointer, the reassignment's free-before-store skips exactly that pointer, and the epilogue then releases it itself if and only if the return was abandoned. That record is one two-field state -- the pending pointer and a flag meaning "the record is its sole owner" -- and it has a second writer: a `return` that supersedes an earlier one in the same frame, reached *sequentially* -- a `return` abandoned by a raising finalizer, caught by an enclosing handler, the name rebound, and a second `return` of the rebound name. The *simultaneous* form -- two returns in flight at once -- is no longer compiled at all: it requires the inner `return` to sit lexically inside a `finally` the outer one's exit path runs, and since review round 5 of [#1164](https://github.com/rotnov/pycc/issues/1164) that is the `C0001` stated in the admission sentence above. The front end's own `L0001` refusal of a bare `return` in a `finally` body could not serve as that check, because it does not survive loop entry: `finally: while True: return a` escapes it, and used to compile into an artifact that segfaulted the hosting interpreter, one pending-return record being asked to hold two. The superseding `return` releases the record it replaces when the frame is that pointer's only owner, and clears the flag, so the record always describes the pointer currently pending. Storing over it raw instead leaked the superseded allocation, one per call, and left the flag describing a pointer that was no longer pending, so the epilogue's trailing release freed the new pointer the slot loop had already released -- an allocator abort inside a plain `extern "C" fn`, which kills the hosting interpreter. Every function **outside the export set** is refused with `C0001`: a private module-level `def`, a specialization, a private method, a method of a private class or of an exception class, an instance method no published constructible class reaches, and a `@property` getter. Membership in the export set is the whole test since [#1174](https://github.com/rotnov/pycc/issues/1174); it was narrower before, excluding every method, because a buffer-returning method's *intra-artifact* call resolves through method-resolution paths the `HirExpr::Call`-keyed buffer-returning-call refusal does not cover, so admitting the declaration would have reached `pycc_codegen`'s `memoryview`-typed call-result panic. #1174 closed those paths instead, at all four resolver exits (`pycc_types`' `buffer::refuse_buffer_returning_method`, called from `class/method_call.rs`, both exits of `class/static_call.rs`, and `class/super_call.rs`), so `g.make()`, `self.make()`, `Grid.make()`, `Grid.build()`, `super().make()` and `Grid[0]` are each a `C0001` at the *call* and the declaration is admitted. A `memoryview` **parameter** may be returned too, as the boundary's *second* return provenance, since Part 1 of [#1175](https://github.com/rotnov/pycc/issues/1175) ([#1178](https://github.com/rotnov/pycc/issues/1178)): a bare `return b` from a public export hands the host a view over storage the **caller** owns. The refusal this replaces was grounded in the wrapper releasing the host's buffer on the way out, and that ground was false. The wrapper now acquires a second, independent buffer export on the argument object with `PyMemoryView_FromObject` **before** it releases its own `Py_buffer`; an export rather than a reference is what pins an exporter's storage, so the returned view spans storage the caller's object still owns. That second export is then checked against the first and refused with a `BufferError` unless it describes the same window -- same address, length, itemsize, `ndim`, `readonly` and format. PEP 688 lets a conforming exporter answer a later `__buffer__` with a *different* window, and nothing routes that later window through the parameter unpack's own `ndim`/format gate, so without the check the host could receive storage the compiled body never operated on, or a shape and format D-244 statement (e) refuses outright. Transferring the *first* export into the returned object instead is unavailable under `Py_LIMITED_API`: `PyMemoryView_FromBuffer` copies the `Py_buffer` without taking ownership of it, so that route would leak the export rather than move it. Refusing an exotic-but-legal exporter is this boundary's existing idiom, the same one that already refuses a conforming two-dimensional or non-`'d'` one. On this path the artifact allocates and frees nothing -- no exporter object is created, `pycc_rt_buffer_f64_free` is never called, and `pycc_rt_buffer_live_views` stays zero for the whole call -- and the returned view's `readonly` is whatever the host's own exporter granted, rather than the artifact-owned path's hard-coded writable. The generated wrapper decides which provenance a given call returned by comparing the returned pointer against each of its own buffer-parameter slots in turn, an exact identity test rather than a heuristic: the slots are distinct live wrapper stack locals, disjoint from any `pycc_rt_buffer_f64_alloc` block. The acquire-before-release order is observable only through a PEP 688 Python-level exporter, and it is the reason the order is fixed: between a release and a later acquire the host object would stand at zero outstanding exports, which is when an exporter that recycles storage on its last release reclaims it. The same preconditions as the artifact-owned egress apply -- definitely assigned, no `return` lexically inside any `finally`. A **sub-range** of a parameter, `return b[start:stop]`, is admitted too, by Part 2 of #1175 ([#1179](https://github.com/rotnov/pycc/issues/1179)): the compiled body returns the same *unnarrowed* pointer, so the identity test and the second-export check above run over the whole window unchanged, and hands the bounds out through three trailing `long long *` out-pointers (`has_slice`, `start`, `stop`) that a forwarding `pycc_ext_thunk_<name>` carries across the aggregate-calling-convention boundary. **Every `return` in such a body writes `has_slice` itself** -- `1` alongside its own bounds, `0` for a whole-window `return` -- so a frame that executes more than one `return` describes the one that actually reached the wrapper; the wrapper's own zero-initialization of the slot is a belt-and-braces default rather than the protocol, and relying on it instead let a sub-range abandoned by a raising finalizer, swallowed by an enclosing handler, colour the later bare `return b` with its bounds. The out-slots are likewise a property of the export **name** over every definition of it rather than of one `def`: two `def`s of one name share one `fnptr_<name>` slot and one compiled signature, so if any definition returns a sub-range every definition carries the out-slots -- last-wins still decides which definition the host calls, and the whole-window one's own `has_slice = 0` store is what makes that come out right. The wrapper then derives the sub-view host-side with `PySlice_New` plus `PyObject_GetItem` on the checked `memoryview`, so negative, absent, inverted and out-of-range bounds follow CPython's own `memoryview` slicing rather than a rule pycc reimplemented, and the artifact still allocates and frees nothing. A bound whose value reaches the slice already promoted to a heap bigint hits the pre-existing `pycc_rt_int_untag_checked` residual this document's D-141 paragraph above already records for *a slice bound*, and aborts rather than raising -- which inside an `ext` artifact aborts the host interpreter. This provenance inherits that residual without widening it: the identical abort is reachable through `list[int]` slicing in an `ext` export without this feature, measured against the pre-#1179 compiler, and the three other routes into the position are already closed at this seam (an argument that is a bigint is refused by the parameter ABI, a literal by `T0051`, and arithmetic on an operand that is already a bigint by D-173). [#1089](https://github.com/rotnov/pycc/issues/1089) tracks migrating the four surviving positions off the aborting decoder. A present `step` is refused in every spelling including the identity `1` -- `PyccExtBufferView` carries no stride -- and so is a slice of artifact-owned storage, a slice in any position other than the returned expression, and returning an intra-artifact call's result, whose value need match no `args[i]` slot of the wrapper that would have to name its owner. An intra-artifact *call* of a buffer-returning function is itself `C0001`: the egress exists for the host, and the artifact has no consumer for the value. `tuple[memoryview]` stays refused at the element level (D-116 `T0039`) |

`float` and `bool` refusing an `int` is not a local choice: it is
`docs/TYPE_SYSTEM.md` rule 4 (D-086), no implicit numeric narrowing *or*
widening at an annotated boundary, which rule 7 defers to for conformance. It
is a deliberate divergence from `PyFloat_AsDouble` and from the C-API
converters' habit of accepting anything convertible; the `str` row refuses
duck types for exactly that reason. A `tuple` is admitted by its elements
and not by its own name: #1050 carries it by spreading it into one scalar
slot per element, so `tuple[list[int]]`, a nested
`tuple[tuple[int], int]`, and `tuple[str, int]` alike stay `C0003` gaps --
a restatement of D-116's model, in which a tuple type has a fixed arity of
`int`/`bool`/`float` elements, rather than a second admissibility rule.
`str` is the one type the two admissibility questions answer differently:
it is carried in both directions at a top-level position (#1049) and is
still not a carriable element, because the element shims take an element
index and exist only for those three. Every other container
remains a `C0003` capability gap for a more durable reason: it is a mutable
heap object with no by-value crossing, not an unimplemented spelling. The
aggregate itself never reaches the generated C. pycc's convention for
passing and returning one is not the platform C struct ABI -- measured on
aarch64-apple-darwin, a pycc function returning
`tuple[int, int, int, int, int]` hands the five words back in `x0`-`x4`
where clang passes a hidden `sret` pointer -- so codegen emits one
`pycc_ext_thunk_<name>` per `tuple`-carrying export whose own signature is
scalars and out-pointers only, the wrapper calls it as a real `extern`
function rather than through a cast, and the aggregate stays on the LLVM
side of the seam. Per the D-244
amendment of 2026-09-12 an `int` outside the inline range `[-2^62, 2^62-1]`
raises `OverflowError` at the wrapper until [#1040](https://github.com/rotnov/pycc/issues/1040)
gives `pycc_rt` a bigint boundary. That guard covers the boundary only, not the
interior: an exported function whose *intermediate* value leaves the inline
range and is then consumed by a further operation (`(x * x) * 0`) used to reach
`require_inline_int`, whose `panic!` crossed a plain `extern "C"` frame and
aborted the hosting interpreter rather than raising; Part C below closed that.
Part 3 of #1025
([#1038](https://github.com/rotnov/pycc/issues/1038)) removes those abort paths
and is a blocker on #1025's closure. Its Part A
([#1063](https://github.com/rotnov/pycc/issues/1063)) has landed the four
numeric-operator `**` paths -- `float_pow`'s three arms and `int_pow`'s
negative exponent now raise `ZeroDivisionError`, `RuntimeError` and
`OverflowError` through D-173 rather than aborting -- so the wrapper epilogue
turns each into a `NULL` return with the exception set. That epilogue covers the
*uncaught* direction only: because `Pow` is not itself a checkpoint, an export
that handles the exception in its own `try` suite observes it at the next
enclosing checkpoint, so a statement following the `**` in that suite runs first
and a second `**` raise before that checkpoint relabels the first. D-244's
2026-09-13 scope amendment records both shapes and why closing them waits on
#1031. Part B
([#1064](https://github.com/rotnov/pycc/issues/1064)) has landed the list, set
and float-formatting paths: `int_list_slice`'s three rejected bounds raise
`ValueError`, `int_list_pop` on an empty list raises `IndexError`,
`float_to_str` outside its `1e-4 <= |x| < 1e16` positional window raises
`RuntimeError`, and `check_set_len_unchanged` raises CPython's own
`RuntimeError: Set changed size during iteration`. None of the six messages
carries the old `pycc_rt: ` panic prefix. The conformance gaps behind the
first and third of those are tracked as
[#1070](https://github.com/rotnov/pycc/issues/1070) and
[#1071](https://github.com/rotnov/pycc/issues/1071). Because a D-173 raise returns
normally where a `panic!` did not, each site also returns a sentinel that is a
*valid* value of its return type -- `tag_smallint(0)`, a fresh empty list, an
empty `str`, never a raw `0` or `NULL` -- and `MirStmt::ForSet`'s loop-test
gained a `pycc_rt_exception_active() == 0` conjunct, without which a `for x in
s: s.add(...)` loop would spin forever instead of reporting the error. That
conjunct gives the loop a second exit edge, so the block after it ends in the
same statement-effect guard every other fallible statement uses: the statement
following a `for` over a set does not run when the loop left through the
exceptional edge. Those
sentinels are observable: `print(1e20)` writes a bare newline to stdout before
the `RuntimeError` reaches stderr, the same "sentinel before the exception is
reported" shape D-244's 2026-09-13 amendment already accepts for Part A. They
are observable in the *assignment* direction too, recorded by a further
2026-09-13 D-244 amendment: neither a slice nor a `.pop()` is an
`expression_can_set_exception` checkpoint, so `result = xs[-1:1]` and
`y = empty.pop()` inside a `try` suite commit their sentinel to the target name
before the suite's checkpoint observes the pending exception, and the handler
sees a binding CPython would have left unchanged. Reaching the loop test with
an exception already pending does *not* relabel it: `check_set_len_unchanged`
returns early when `pycc_rt_exception_active()` is non-zero, so a body that
both grows the set and raises propagates its own exception rather than the
resize check's `RuntimeError`. Part C
([#1065](https://github.com/rotnov/pycc/issues/1065)) has landed the
bigint-intermediate paths: `require_inline_int` is gone, replaced by
`decode_inline_or_raise`, which raises `OverflowError` and yields `None`, and
each of its seven callers -- `int_mul`, `int_floordiv`, `int_floormod`,
`int_pow`, `int_cmp`, `int_to_float` and `pycc_rt_int_set_add` -- returns its
own type-valid sentinel (`tag_smallint(0)`, a plain `0` ordering, `0.0`, or no
value) immediately. Returning immediately is the correctness condition, not a
style choice: `raise_builtin` installs unconditionally, so a bigint divisor
allowed to fall through as a decoded `0` would have reported
`ZeroDivisionError` over the `OverflowError`, and a bigint exponent would have
computed `1`. `int_pow`'s own two checks do not cover its loop: `int_mul`
promotes an overflowing product to a heap bigint, so `(2 ** 40) ** 2` raises with
the `multiplying` context even though its only operator is `**` -- pre-existing
in kind (the retired `panic!` said the same) and now observable because the
abort became a returning raise. That promotion also makes `int_pow` the one caller in the list with heap temporaries of its own, so it releases both its accumulator and its squared base and stops on the pending exception instead of squaring on: before Part C the aborting process reclaimed them, whereas a host that catches the `OverflowError` in a loop would otherwise leak one `BigIntObj` per attempt. Both releases are unconditional and need no ownership flag -- the accumulator starts as a smallint and the base was already proved inline, so a bigint in either can only be one `int_mul` allocated here, and `bigint_release` no-ops on every inline kind. This is not a general temporary-ownership model: unbound arithmetic temporaries elsewhere still leak, which stays #146 Part 2 ([#625](https://github.com/rotnov/pycc/issues/625)). One shape of that gap is newly *reachable* for the same reason and is tracked separately as [#1076](https://github.com/rotnov/pycc/issues/1076): the three consumers that convert an `int` through `pycc_rt_int_to_float` -- the float-typed `BinOp` arm, builtin `float(...)`, and float comparison -- do not call `release_if_int_temporary` on the source expression, so a promoted operand, which always raises there, leaks one `BigIntObj` per evaluation in a host that catches the `OverflowError`. That release belongs to `pycc_codegen`'s site classification rather than to the runtime, which is why it is a separate change. The same missing guard has a second, non-leak direction recorded on that issue: codegen emits both `pycc_rt_int_to_float` conversions and the float runtime operation as one unguarded sequence, so when a promoted operand raises `OverflowError` and the *other* operand is a real zero, `float_div`'s own `ZeroDivisionError` overwrites the pending state and the wrong exception class is observed. `decode_inline_or_raise`'s first-raise-wins guarantee is scoped to a single runtime call, not across a sequence codegen composes, so closing this needs the same codegen-side exception-active check as the leak. Part C needed no codegen change, because the `ext` wrapper
already emits its pending-exception check ahead of every return arm, so no
packer reads a sentinel. The residual is the same accepted one as Parts A and
B: `Mul`, `Pow` and `Compare` are not `expression_can_set_exception`
checkpoints, so `print(big * 2)` writes the `0` sentinel before the exception
is reported, while `print(big // 2)` -- `FloorDiv` *is* a checkpoint -- writes
nothing. `Add` and `Sub` join that list in their float-typed form only: a
mixed-operand `BinOp` whose result type is `Float` converts *both* operands
through `pycc_rt_int_to_float` before it dispatches on the operator, so
`print(big + 1.5)` writes `1.5` -- the `0.0` sentinel plus the literal --
before the same `OverflowError` is reported. An `ext` artifact whose arithmetic leaves the inline range now reports
a catchable `OverflowError` to its host rather than killing the interpreter;
what it still does not do is compute the bigint result, which is #1040.
Separately, D-141's own runtime `int` boundary
(`pycc_rt_int_untag_checked`: a container value, a list index, a `str` repeat
count, a slice bound) is untouched by Part C and still aborts. Those four are
the whole of the aborting set *among the `int`-boundary positions*: the
`ndarray(n)` length, which Part 2a of #1142
([#1165](https://github.com/rotnov/pycc/issues/1165)) added after Part C, is
deliberately decoded by `pycc_rt_buffer_alloc_untag_len` instead and raises.
That count is not an inventory of every way this runtime can abort: an
infallible allocation is a separate class, and one that no `int` boundary is
involved in. #1166's round-11 review closed the one such allocation on the
`ndarray(n)` path -- the `PyccExtBufferView` itself, now reserved fallibly
beside its elements -- while every raise site in this runtime, including the
two that path uses, still builds its message and exception objects
infallibly, so a genuine out-of-memory condition can still abort inside the
raise that was meant to report it. Closing that is runtime-wide and is not
#1165's. That boundary is
emitted *ahead* of `pycc_rt_int_set_add`, so a compiled `s.add(v)` with a
bigint `v` aborts there before reaching the converted guard, which remains as
defense-in-depth for a direct ABI caller. An exception that
escapes an export is re-raised as the matching CPython class for every
builtin class the bridge carries a tag for (all of them except the two PEP 654
groups, which cross as `Exception`). A *user-defined* exception class
keeps its identity as of Part D of #1038 (#1066): the artifact synthesizes one
CPython class per such class at import time, parented on the same bases the
source declares, so `except m.MyError:`, `except ValueError:` for a subclass of
a builtin, `type(e).__name__`, `type(e).__module__` and `e.args` all behave as
they do for a hand-written extension. The two PEP 654 group classes still reach
the caller as `Exception` with the original message, and so does any user class
derived from one: the limited C API exposes no `PyExc_ExceptionGroup`, and PEP
654 requires `(msg, exceptions)`, so a synthesized stand-in would be a fake
group class rather than CPython's; carrying a real group across the boundary
needs the `exceptions` sequence itself to cross and is tracked as
[#1073](https://github.com/rotnov/pycc/issues/1073). Foreign imports,
opaque objects, and the buffer protocol are Parts 2-4. That mode's typed boundary
additionally faces callers pycc does not compile, so what a typed export
wrapper does with an argument that violates its annotation is D-244 rule 7 —
the oracle is scoped to annotation-conforming calls and the wrapper raises
`TypeError` outside them — and it is not restated here.

Two properties of that `ext` boundary are deliberate narrowings rather than
oversights, and a caller that relies on CPython's own behavior will see a
difference. First, the boundary carries scalar *values*, not objects: an `int`
subclass instance is accepted and decoded, so `int`-subclass identity does not
survive a crossing and `echo(E.X) is E.X` is `False` where an equivalent CPython
function gives `True`. Preserving it is not implementable over D-061/D-141's
unboxed tagged word, and narrowing the accepted domain to exact `int` instead
would reject `bool`, which CPython accepts and which the #1036 oracle asserts;
[#1043](https://github.com/rotnov/pycc/issues/1043) tracks whether an
object-carrying path is worth its cost. #1048 added a second instance of that
same question rather than a new one: the `float` parameter's `PyFloat_Check`
likewise accepts a `float` subclass and flattens it to a `double`, so
`float`-subclass identity does not survive either. `bool` is exempt, being
unsubclassable. #1049 extends the same narrowing to `str`, and widens it: a
`str` subclass is accepted by `PyUnicode_Check` and flattened to a plain pycc
`str`, and identity does not survive *even for an exact `str`* — the boundary
copies the UTF-8 bytes in each direction, so `m.echo(s) is s` is `False` where
an equivalent CPython function gives `True`. That makes #1043 a question about
the `str` boundary too, not only the numeric one. #1050 extends the same
narrowing to `tuple` and closes it over the container: a `tuple` subclass
(a `collections.namedtuple` included) is accepted by `PyTuple_Check` and
its elements are copied out by value, so what comes back is always an exact
`tuple` and identity never survives -- for a subclass, and for an exact
`tuple` handed to an identity export alike. The synthesized exception classes
of #1066 are a third narrowing of the same kind, recorded in the same register:
the synthesized class carries only the *exception* bases its source declares, so a
method-only mixin (`class E(Mixin, ValueError)`) is dropped and
`isinstance(e, Mixin)` is `False` host-side where native pycc holds it; and a
class deriving from a PEP 654 group is not synthesized at all, so it arrives as
`Exception` exactly as the group classes themselves do. Instance state beyond
`args` is not a narrowing of this boundary at all: a class with its own
`__init__` is rejected as `C0001` at every raise and `except` site in both
modes (`pycc_types::exception::reject_own_constructor`, the #541 Part 3 gap),
so no such instance exists to cross it. Such a class is still tagged, and so
still registered as a module attribute the host can name. Second,
an `ext` module's state is process-static — generated globals live in LLVM
globals and the `METH_FASTCALL` wrappers ignore their module argument, with
`m_size = 0` and no `m_free` — so PEP 489's per-instance guarantee does not
hold. A subinterpreter is refused outright
(`Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED`), and `importlib.reload()` is
unaffected because CPython does not re-run `Py_mod_exec` for an extension
module; the one divergent path is deleting the `sys.modules` entry and
importing again, which re-runs the module body and lets the second instance
overwrite state the first instance's wrappers still read. The synthesized
exception classes sit inside that same contract: they are created once in
`Py_mod_exec`, published as module attributes with `PyModule_AddObjectRef`, and
held by a file-scope cache that the shim's refusal of subinterpreters and
free-threaded hosts licenses. A re-exec on that divergent path **reuses** the
cached classes rather than minting new ones, so class identity stays stable
across it and the first instance's classes are not leaked; a failure to create
or publish one fails the import rather than importing a module whose
`except m.MyError:` would silently never match.

[#1044](https://github.com/rotnov/pycc/issues/1044) carries the choice between
rejecting that second instance and allocating state per instance.

### Foreign imports in the module body

Part 1 of [#1026](https://github.com/rotnov/pycc/issues/1026) makes a plain,
undotted `import <name>` a *foreign* import when `<name>` is neither
a project module nor a `pycc_std` registration, and [#1291](https://github.com/rotnov/pycc/issues/1291) extends that to the
aliased `import <name> as <alias>`: it binds the CPython module
object itself (to `<alias>` when there is one), typed `object` (see
[TYPE_SYSTEM.md](./TYPE_SYSTEM.md)'s representations table). Part 2 of the same
issue adds a second producer — an attribute load on such a value, `numpy.pi`,
which is itself an `object` — but the admissibility table above is still why one
can never leave: an `object` parameter or return on an exported function is a
`C0003` capability gap, so the value stays inside the artifact.

**Position, not a prologue.** D-244 rule 3 binds the artifact to CPython's
statement-by-statement module body, so each foreign import runs *where it was
written*. `pycc_mir::build` splices one `MirItem::ForeignImport` into the item
list at the import statement's own recorded position rather than hoisting every
import to the top of `Py_mod_exec`; a module-level statement with an observable
effect written above a failing import therefore has already run when the import
raises, exactly as under CPython. Each qualifying name of a multi-name
`import a, b` is its own foreign import (#1280), and the names run in source
order at the statement's position, so the first missing one raises and the
names after it are never imported (`tests/issue_1280_multi_import.rs`).
Since [#1291](https://github.com/rotnov/pycc/issues/1291) a foreign import may also stand inside a module-level `if` or `try`
block (including `elif`, `else`, `except`, `except*` and `finally` bodies, at
any nesting depth of those blocks, but not inside a function, a loop, a
`with` or a `match`). It lowers to a `MirStmt::ForeignImport` in place in that block rather
than to a spliced `MirItem`, so it runs only if control reaches it: a branch
that is not taken imports nothing, and a missing module in a taken one raises
from that statement (`tests/issue_1291_block_import.rs`). Since
[#1293](https://github.com/rotnov/pycc/issues/1293) such a failure can be
caught. When the import raises an `ImportError`, the shim's
`pycc_ext_import_error_bridge` translates it into a pending pycc exception
before anything else runs: a `ModuleNotFoundError` (or a subclass of it)
becomes pycc's `ModuleNotFoundError` (tag 27), any other `ImportError` becomes
`ImportError` (tag 26), and the message is CPython's own `str(exc)`. The
generated code then branches to the innermost exception target exactly as an
explicit `raise` does, so an enclosing `except ImportError`,
`except ModuleNotFoundError`, `except Exception`, bare `except`, `except*` or
`finally` runs as under CPython, and the statements after the import in the
same body do not (`tests/issue_1293_import_bridge.rs`). The shim keeps each
bridged pycc exception paired with a strong reference to CPython's original in
a per-exec bridge table. If that pycc exception escapes the module body
unchanged through a plain `try`/`except`/`finally` -- unmatched, re-raised with
a bare `raise`, or re-raised after `finally` -- `pycc_ext_raise_pending` finds
it there and re-raises the *original* object rather than a rebuilt one, so the
host still sees its `.name`, `.path` and exact class, and an embedded
executable's uncaught output is unchanged. The table is emptied when
`Py_mod_exec` returns. Two bounds remain. An escape through an `except*`
statement re-raises the exception group pycc wrapped the exception in, which is
not in the table, so the host receives a rebuilt `Exception` carrying the
import's message (a recorded deviation). And an import that fails with anything
other than an `ImportError` -- the imported module body's own `ValueError`, a
`SyntaxError`, a `BaseException` such as `KeyboardInterrupt` -- is not bridged:
`Py_mod_exec` returns `-1` directly with CPython's exception untouched, so an
enclosing `except` or `finally` body does **not** run for it (the remaining
[#1096](https://github.com/rotnov/pycc/issues/1096) deviation). A top-level
foreign import has no enclosing handler and keeps that direct edge too.
`tests/issue_1080_foreign_object.rs` asserts
that against a real host interpreter, and
`crates/pycc_codegen/src/foreign_import.rs`'s own tests assert it at the
emission layer.

**Failure.** The emitted call is `pycc_ext_obj_import`, which wraps
`PyImport_ImportModule`. A failure leaves CPython's own exception set — a
missing module surfaces to the host as `ModuleNotFoundError` naming the module,
not as a pycc diagnostic and not as an abort — and `Py_mod_exec` returns `-1`,
so the import statement that loaded the artifact fails and no partially
initialized module is left in `sys.modules`. A nested import's `ImportError`
is translated into a pycc exception first (#1293, above); when it goes
uncaught, the host still receives CPython's original exception object.

An attribute load fails the same way and takes the same edge.
`pycc_ext_obj_getattr` returns `NULL` with CPython's error indicator set, and
`crates/pycc_codegen/src/foreign_attr.rs` tests that result and returns `-1`
from `Py_mod_exec` immediately, leaving the exception untouched, so a missing
attribute surfaces to the host as the real `AttributeError` and the remaining
module-body statements never run. That check is what pycc's *own*
pending-exception guard (D-173) cannot do: the two failure protocols are
separate, and pycc's state is unset while CPython's is set, so before PR 2a of
[#1081](https://github.com/rotnov/pycc/pull/1093) the body ran to completion and
CPython reported `SystemError: execution of module <name> raised unreported
exception` instead. Translating CPython's exception into pycc's pending state —
which is what a function body would need, since only the module-body entry point
may return `-1` — is not implemented, and the type checker refuses reading a
foreign object anywhere but a module body ([TYPE_SYSTEM.md](./TYPE_SYSTEM.md)).

**A method call fails on that same edge, and inherits that same bound.** PR 2b
of [#1081](https://github.com/rotnov/pycc/issues/1081) added
`MirExpr::ObjMethodCall`, which emits two shim calls in CPython's own
evaluation order: `pycc_ext_obj_getattr` resolves the method *before* the
argument expressions are evaluated, then the packed arguments and the resolved
callable go to `pycc_ext_obj_call`, which does the `PyObject_Vectorcall`.
Resolving first is observable and required — `obj.missing(1 // 0)` must raise
`AttributeError`, not `ZeroDivisionError`. Either shim call returns `NULL` with
CPython's error indicator set, and
`crates/pycc_codegen/src/foreign_call.rs` tests each result exactly as
`foreign_attr.rs` does and returns `-1` from `Py_mod_exec`, so a missing method
surfaces as the real `AttributeError` and a method that raises surfaces its own
exception, with the remaining module-body statements never running. Because a
call is admitted only in a module body — the same positional rule that governs
a load, for the same two reasons — PR 2b needed no CPython-to-pycc exception
bridge at all: the one function a call can appear in is the one function with a
`-1` edge. Lifting the bound is what would require the bridge, alongside the
ordering-aware name resolution TYPE_SYSTEM.md describes.

**`len` and a truth test fail on that same edge, and inherit that same bound.**
PR 3a of [#1082](https://github.com/rotnov/pycc/issues/1082) added two more shim
helpers, and each reports failure as `-1` rather than as `NULL`, because each
answers a scalar rather than a `PyObject *`. `pycc_ext_obj_len` wraps
`PyObject_Size` and then D-141-encodes the result, so an operand with no
`__len__` surfaces as CPython's own `TypeError` — the encode is fused into the
same helper deliberately, so codegen emits one failure edge for `len` rather
than two, and the encode arm is unreachable for a real container.
`pycc_ext_obj_truthy` wraps `PyObject_IsTrue`, which calls the operand's own
`__bool__` or `__len__` and so really can raise arbitrary user exceptions.
`crates/pycc_codegen/src/foreign_len.rs` tests each status and returns `-1` from
`Py_mod_exec`, exactly as `foreign_attr.rs` does for a `NULL`. Both are
admitted only in a module body, on the identical positional rule and for the
identical reason, so neither needs the exception bridge either.

**A subscript load fails on that same edge too, and inherits that same bound.**
PR 3b of [#1082](https://github.com/rotnov/pycc/issues/1082) added one more shim
helper, `pycc_ext_obj_getitem`, which wraps `PyObject_GetItem` and — answering a
`PyObject *` rather than a scalar — reports failure as `NULL`, like the two
helpers above it rather than like the two below. `o[k]` therefore surfaces the
host's own `KeyError`, `IndexError` or `TypeError`, and the remaining
module-body statements never run.
`crates/pycc_codegen/src/foreign_call.rs` emits exactly **one** failure edge for
the whole operation: the helper tolerates a `NULL` key and answers `NULL`
itself, so a failed key packer needs no branch of its own, the same fusing
`pycc_ext_obj_len`'s encode arm uses for the same reason. The key is restricted
to the four packable scalars, so the packer choice is total. The load is
admitted only in a module body, on the identical positional rule and for the
identical reason, so it needs no exception bridge either.

**A `for` loop fails on that same edge twice, and inherits that same bound.**
PR 3c of [#1082](https://github.com/rotnov/pycc/issues/1082) added the last two
shim helpers. `pycc_ext_obj_get_iter` wraps `PyObject_GetIter` and reports
failure as `NULL`, so iterating a non-iterable surfaces the host's own
`TypeError`. `pycc_ext_obj_iter_next` is the one helper with a **three-valued**
contract: `long long pycc_ext_obj_iter_next(PyObject *it, PyObject **out)`
returns `1` with the next item written to `*out`, `0` for clean exhaustion, and
`-1` for an error. `PyIter_Next` answers `NULL` for both exhaustion and
failure, and only `PyErr_Occurred()` tells the two apart; that discrimination
lives **inside the C helper** rather than in emitted IR, so the compiler emits
one three-way switch over a status word instead of reproducing a CPython
protocol rule per call site.
`crates/pycc_codegen/src/foreign_call.rs` lowers the loop into the blocks its
own `a_foreign_for_loop_emits_its_full_block_structure` test enumerates, which is
the authority for the exact list: `get_iter` runs in the current block and its
`NULL` edge goes through the shared `fail_on_null` helper, which appends the
`foreign_iter_get_fail` / `foreign_iter_get_cont` pair every foreign call
already uses; then come `foreign_iter_header`, which calls `iter_next` and
switches `-1` to `foreign_iter_next_fail`, `0` to `foreign_iter_after` and `1`
to `foreign_iter_body`; `foreign_iter_body`; `foreign_iter_after`; and
`foreign_iter_next_fail`. **Exhaustion is not a failure edge**: an empty iterable runs the body zero
times and the module body continues. The loop therefore adds exactly **two**
new `-1` returns from `Py_mod_exec` — one for a non-iterable, one for an
iterator that raises mid-iteration — and no more.
The out-parameter the item is written through is a single pointer slot hoisted
into the module-exec entry block, so a loop does not grow the host's stack. The
loop is admitted only in a module body, on the identical positional rule and
for the identical reason, so it needs no exception bridge either.

**The `float` and `bool` conversions fail on that same edge, and inherit that
same bound.** PR 4a of [#1083](https://github.com/rotnov/pycc/issues/1083)
(Part 4 of #1026) added exactly one shim helper, `pycc_ext_obj_to_float`, which
wraps `PyNumber_Float`, reads the result with `PyFloat_AsDouble`, writes the
`double` through an out-parameter and reports failure as `-1` — so it joins
`len` and the truth test in `crates/pycc_codegen/src/foreign_len.rs` rather than
the `NULL`-answering helpers. An operand with no `__float__`/`__index__` and no
parseable text surfaces the host's own `TypeError` or `ValueError`, and the
remaining module-body statements never run. `bool(o)` adds **no helper at all**:
it is the `pycc_ext_obj_truthy` call the truth test already makes, widened from
`i1` to the byte a pycc `bool` occupies, so it inherits that helper's failure
edge unchanged, and it has no out-parameter of its own. The `double`
out-parameter belongs to `pycc_ext_obj_to_float` alone: a single slot hoisted
into the module-exec entry block, on the same rule `len`'s `i64` slot follows,
so a module-scope loop around a `float(o)` does not grow the host's stack. Both are
admitted only in a module body, on the identical positional rule and for the
identical reason, so neither needs the exception bridge either.

Running CPython's own conversion protocol here is **not** a D-244 rule 7
violation. Rule 7 keeps the type boundary closed at the *thunk export seam*,
where a value crosses implicitly and its annotation is the whole contract.
`float(o)` in user source is an explicit conversion request that names its
destination type, so running the operand's own `__float__` is what the author
asked for. [TYPE_SYSTEM.md](./TYPE_SYSTEM.md)'s `object` row carries the
user-facing statement of the same distinction.

**The `int` and `str` conversions join them on that edge.** PR 4b of
[#1083](https://github.com/rotnov/pycc/issues/1083) added two more shim helpers,
`pycc_ext_obj_to_int` and `pycc_ext_obj_to_str`, both shaped exactly like
`pycc_ext_obj_to_float`: one out-parameter, `-1` with a CPython exception set on
failure, and a single entry-block slot per conversion kind. `pycc_ext_obj_to_int`
runs `PyNumber_Long` and reads the result with `PyLong_AsLongLongAndOverflow`,
then encodes it with `pycc_rt_ext_int_encode`; a value outside the inline-integer
range `[-2**62, 2**62-1]` raises `OverflowError` naming
[#1040](https://github.com/rotnov/pycc/issues/1040), the same refusal and the
same range the thunk boundary already applies, since there is no bigint path
across this boundary. `pycc_ext_obj_to_str` runs `PyObject_Str` and
`PyUnicode_AsUTF8AndSize`, then copies the bytes with `pycc_rt_str_from_literal`
— the identical call a `str` literal compiles to — so the result is an ordinary
pycc `str` with no new ownership rule; a lone surrogate, which has no UTF-8
form, surfaces CPython's own `UnicodeEncodeError` on that same failing edge.
Neither is the thunk seam's unpacker: `pycc_ext_unpack_int_at` and
`pycc_ext_unpack_str` type-check their operand precisely because that seam is
closed, and refusing a duck type is what an explicit conversion must not do.
The rule-7 paragraph above covers all four conversions unchanged.

**The `tuple` unpack is the sixth helper on that edge, and the first with a
non-scalar out-slot.** PR 4c of
[#1083](https://github.com/rotnov/pycc/issues/1083) added exactly one shim
helper, `pycc_ext_obj_unpack_float_tuple`, which admits a module-level
annotated assignment of an `object` to a fixed-arity all-`float` `tuple`. Its
admission rule is **strict container, converting elements**: it checks
`PyTuple_Check` — not `CheckExact`, so a structseq such as `sys.version_info`
is admitted — with exactly the declared arity, then converts each item with
`PyNumber_Float` and writes the `double`s through an out-parameter. The
container half is strict because D-115/D-116 fix the destination's shape at
compile time and no arity makes a differently sized sequence representable;
the element half converts for the reason the rule-7 paragraph above states,
so the items are never type-checked and a non-numeric one surfaces CPython's
own exception. The arity is a `long long` **parameter**, never a constant, so
one helper serves every admitted arity. Failure is reported as `-1` with a
CPython exception set, on the same unconditional `Py_mod_exec` edge every
helper in this section uses, so the unpack adds exactly **three** new `-1`
returns — a non-tuple operand, a wrong-length tuple, and an item CPython
declines to convert — and no more. The out-parameter is an `[arity x double]`
array hoisted as a single slot into the module-exec entry block, on the rule
`len`'s `i64` slot follows, so a module-scope loop around one does not grow
the host's stack; codegen then rebuilds the D-115/D-116 by-value struct from
that slot with `insertvalue`. Like every other operation in this section it
is admitted only in a module body, on the identical positional rule, so it
needs no exception bridge either.

**Ownership.** `pycc_ext_obj_import` returns the *new* reference
`PyImport_ImportModule` hands back and the artifact never releases it: the
module object is reachable from `sys.modules` for the life of the interpreter
regardless, and the `object` binding is a module-level global with no scope to
leave. Part 2 of #1026 keeps that rule and extends it to the values an
attribute load produces: `pycc_ext_obj_getattr` wraps `PyObject_GetAttrString`,
whose result is also a new reference, and it too is never released. A method
call's *result* is governed by the same rule for the same reason:
`PyObject_Vectorcall` hands back a new reference and `pycc_ext_obj_call`
returns it to compiled code unreleased. So is a subscript load's:
`PyObject_GetItem` hands back a new reference and `pycc_ext_obj_getitem`
returns it unreleased. Iteration adds two producers on the same terms:
`PyObject_GetIter` hands back a new reference to the iterator, leaked once per
loop, and `PyIter_Next` hands back a new reference to each item, which
`pycc_ext_obj_iter_next` writes through `*out` unreleased — so **`for` makes
the leak trip-count-linear by construction**, where an attribute load in a loop
body merely happens to be written inside one.

**The key slot repeats the argument slot's rule rather than inventing a second
one.** `pycc_ext_obj_getitem` *borrows* the object and **consumes the key
reference on every path**, including the one where either argument is already
`NULL` — exactly as `pycc_ext_obj_call` consumes each element of the packed
argument array on every path, including the early one where a packer failed.
The four packers therefore keep one contract at both call sites: each borrows
its pycc-side value, returns a new reference, and the shim helper it is handed
to owns that reference from then on. Codegen consequently emits no release of
its own around a subscript load, and a failed packer's `NULL` is safe to pass
straight through.

`len`, a truth test, Part 4's four conversions and Part 4's tuple unpack are
the operations that add nothing to that leaked set. `pycc_ext_obj_len` answers a `Py_ssize_t` and
`pycc_ext_obj_truthy` answers a C `int`; neither creates a reference and neither
touches the operand's refcount on any path, so `len(o)` or `if o:` inside a
module-scope loop is refcount-neutral no matter the trip count. The
out-parameter `len` writes through is a single `i64` slot hoisted into the
module-exec entry block, so such a loop does not grow the host's stack either.
`pycc_ext_obj_to_float` is the first helper that *does* create a CPython
temporary — `PyNumber_Float` hands back a new reference — and it is also the
first that **releases what it owns on every exit, not only the successful
one**: the `Py_DECREF` runs before the failing return as well, so a raising
`PyFloat_AsDouble` leaks nothing either. Only a `double` escapes into compiled
code. PR 4b's `pycc_ext_obj_to_int` and `pycc_ext_obj_to_str` follow that rule
exactly, each releasing its `PyNumber_Long`/`PyObject_Str` temporary on the
failing return as well as the successful one; `pycc_ext_obj_to_str` carries the
one additional ordering constraint, that the `pycc_rt_str_from_literal` copy
must complete **before** the `Py_DECREF`, because `PyUnicode_AsUTF8AndSize`
points into the temporary's own buffer and that buffer dies with it. Only an
encoded `i64` and a pycc-owned `str` handle escape into compiled code. PR
4c's `pycc_ext_obj_unpack_float_tuple` follows the same rule once per item:
each `PyNumber_Float` temporary is released inside the iteration that
produced it, before the next item is read and before any failing return, so
the failing exit holds nothing and an operand of any arity leaves no
reference behind. It borrows its operand, whose items it reads with the
borrowing `PyTuple_GetItem`, and only plain `double`s escape into compiled
code.
**Part 4 therefore does not grow
[#1092](https://github.com/rotnov/pycc/issues/1092)**, and
`float(o)`/`bool(o)`/`int(o)`/`str(o)` in a module-scope loop hold no CPython
reference at any trip count. `str(o)`'s pycc-side `str` handle is a separate
matter: a *bound* one is retired by the ordinary store protocol, but a
*discarded* one — `str(o)` in statement position — is never retired, because
`MirStmt::ExprStmt` releases an `int` temporary and has no `str` counterpart.
That is the general behavior of every discarded `str` temporary rather than
anything Part 4 introduces (`a + b` in statement position leaks identically,
measured at roughly 97 bytes per trip over 2,000,000 trips), and it is tracked
as [#1109](https://github.com/rotnov/pycc/issues/1109).

Everything the call creates *internally*, by contrast, is released, so the leak
is exactly one reference per call rather than one per argument plus two.
`pycc_ext_obj_call` owns the bound method object `pycc_ext_obj_getattr`
produced and `Py_XDECREF`s it on every path, and it consumes the packed
argument array — releasing each element on every path, including the early one
where a packer failed. That release is measured, not merely asserted: one
million calls passing a *named* `str` grow the resident set no faster than one
million calls passing an `int`, once the leaked result is accounted for. One argument shape has no CPython value to build, and it
raises rather than aborting: a D-141 heap-bigint `int` word reaching
`pycc_ext_obj_pack_int` sets `OverflowError` naming the inline range and
returns `NULL`, which the shim treats exactly as a failed call -- it skips the
vectorcall, and the module body stops. That is the
same boundary narrowing the `ext` export ABI already applies to an `int`
parameter or return, on the same terms and until the same issue
([#1040](https://github.com/rotnov/pycc/issues/1040)) widens it; the type
checker cannot pre-empt it, because only the run-time word distinguishes a
bigint from any other `int`. The four argument packers
(`pycc_ext_obj_pack_int`, `_pack_float`, `_pack_bool`, `_pack_str`) *borrow*
their pycc-side input: each builds a new CPython object from the pycc value and
leaves the pycc value alone. That is deliberately the opposite of the `str`
*result* packer D-244's 2026-09-13 `str`-ingress amendment describes, which
consumes the `PyStrObj` it is handed, and it is what lets the same `str`
local be passed to two calls in a row: were the argument packer to consume, the
second call would read freed memory. Nothing crossing into the shim transfers
ownership, so codegen needs no D-208 `pending_int_releases` bookkeeping around
a call — the mark/truncate pair that exists for pycc's own int temporaries has
nothing to guard here.

That extension is a deliberate, bounded regression and is recorded as one. A
module object leaks at most once per process; an attribute load sits inside
ordinary control flow, so `numpy.pi` written in a loop leaks one reference per
iteration — the leak is trip-count-linear rather than bounded by process exit.
Part 2 accepts it because releasing correctly requires a release protocol that
is not yet built, and because nothing in Part 2 can hand such a value to a host:
every consuming operation other than a further attribute load, a method call,
a subscript load, `for` iteration, `len`, a truth test or a
`float`/`bool`/`int`/`str` conversion is refused with `I0404`, and the `ext` export boundary refuses an `object`
parameter or return (`C0003`). A method call's result leaks on exactly the same
terms and is trip-count-linear in exactly the same way. **A benchmark run under
[D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
rule 6's 5× kill criterion must not measure a hot loop containing a foreign
attribute load, a foreign method call or a foreign subscript load until the
release protocol lands, and must not measure a foreign `for` loop at all**,
since that one leaks an item per trip whatever its body contains. **The caveat
does not extend to Part 4's conversions**: `float(o)`, `bool(o)` and `int(o)`
produce only a `double` or an encoded `i64` and leak nothing, so a hot loop
containing only those is a legitimate measurement. `str(o)` qualifies only when
its result is bound; a discarded `str(o)` leaks its pycc `str` handle on
[#1109](https://github.com/rotnov/pycc/issues/1109)'s general terms, exactly as
a discarded `a + b` does, so a benchmark binds the conversion's result rather
than discarding it --
because the resident-set
growth, not the compiled code, would dominate the result. An *unbound* `str`
argument expression — `json.dumps(a + a)` rather than `json.dumps(s)` — adds a
second, unrelated growth term on top: pycc's own pre-existing unbound-`str`-temporary
leak, which the "Language surface" row of [ROADMAP.md](./ROADMAP.md) already
records and which a native build with no foreign call reproduces identically.
It is not owned by this boundary, but it compounds here, so prefer a named
`str` local when measuring. [Issue #1092](https://github.com/rotnov/pycc/issues/1092)
tracks releasing object temporaries.

**The loop shape these parts were built for is now covered end to end, and
the cost is measured rather than estimated.** Part 5 of
[#1026](https://github.com/rotnov/pycc/issues/1026) adds two harnesses.
`tests/issue_1084_loop_shape.rs` compiles a module-scope `for` loop that
calls a method on a foreign object, unpacks its result into a
`tuple[float, float, float]` and accumulates a `float`, then compares the
extension module's answer against CPython's own execution of the identical
source under the identical interpreter; the two agree byte for byte. That
oracle is admissible for that subject specifically because its exported
thunk takes no arguments and so never reaches
[D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md)
rule 7's NEG-005 deviation. `tests/issue_1084_refcount_probe.rs` measures
the price with `sys.getrefcount`, and states it as a differential: across a
loop of `N` trips the reference count of a foreign module attribute grows
by exactly `producing_operations * N`, where a producing operation is an
attribute load. Measured per trip over 1000 and 2000 trips, and again for a
lone `len` loop that isolates the producing side: `float(o)`, `bool(o)`,
`int(o)`, `str(o)`, `len(o)` and the fixed-arity tuple unpack each add
**+1** for the attribute load that feeds them and **+0** for the conversion
itself. The instrument reads the foreign module's own attributes, so what it
shows is that the conversion adds nothing *there*; a helper's internal
temporaries — `pycc_ext_obj_unpack_float_tuple`'s per-element
`PyNumber_Float`, say — stay covered by construction, as the D-244
amendments record them. The consuming operation is therefore not what leaks
across the boundary — #1092 is,
and its closing change is expected to edit that test, since every absolute
in it becomes `0`. A `#[cfg(unix)]` peak-RSS arm covers the blind spot the
refcount instrument leaves, checking that a hundredfold increase in trips
does not move the resident set on pycc's own heap for a *bound* `str(o)`;
a discarded one stays #1109's, as above.

The same rule decides what a *duplicate* foreign import does, and that
outcome is a recorded decision rather than an unexercised side effect. A
module's imports are not definitions to `pycc_hir::program::link`, so two
linked project modules may each write `import numpy`; each contributes its
own `MirItem::ForeignImport`, while `pycc_codegen` keys the foreign-import
globals by local name and so gives both the same single slot. Both calls
run, in linked-program order (the concatenation `link` produces, not either
module's own source order), and the second overwrites the slot with its own
new reference. That is correct by the rule above rather than in spite of it:
the slot ends up holding a valid, correctly typed module object, and the
first reference is simply never released — exactly what every foreign
import does. Collapsing the duplicate to one call, or releasing the
overwritten reference, would be an optimization of an already-correct
program, and belongs with the release protocol described above.

**The bound name does not cross a module boundary yet.** The binding is
positional — a top-level `ImportBinding::Foreign` carries the index of the
item the import sits at in *its own* module's item list, which
`program::link` rebases onto the linked program, and a block one
([#1291](https://github.com/rotnov/pycc/issues/1291)) carries no index at all, its position being the block statement
itself — so it is meaningful only in the module that wrote the `import`. Two consequences are refused rather than
approximated, both `C0001` while lowering, so `pycc check` reports them and
neither build path is reached:

- **Re-export.** `from dep import numpy`, where `numpy` is `dep.py`'s own
  foreign import, is refused at the importing statement. The importer
  produces no item for that statement, so there is no position in its item
  list that could carry the binding honestly; cloning `dep.py`'s index into
  the importer would both mis-rebase it and run a second
  `pycc_ext_obj_import` for one source statement.
- **Cross-module shadowing.** A top-level definition of a name that a
  *different* linked module binds as a foreign import is refused at the
  definition, in either dependency order. Part 1 of #881 links modules into
  one flat namespace, so `dep.py`'s `import numpy` and `main.py`'s
  `def numpy()` would otherwise make `dep.py`'s own `numpy(...)` resolve to
  the entry module's function instead of raising CPython's `TypeError`. A
  module shadowing its *own* foreign import is refused by the same rule and
  in the same phase, in either order -- see below.

**A module does not shadow its own foreign import either.** A module in which
any other top-level statement binds a foreign import's local name -- a `def`,
a `class`, a `type` alias, a plain assignment, or a second `import`, written
above or below the import -- is refused with `C0001` while lowering, at the
shadowing statement, or at the import itself when the shadowing binding is
another import and so has no statement span of its own. Two foreign imports
that bind one local name to the *same* module (`import numpy` twice, or in
both arms of an `if`/`else`) are exempt since [#1291](https://github.com/rotnov/pycc/issues/1291): each produces the same
module object at the same type, so no read depends on which one ran last, and
the second store simply overwrites the slot with a new reference, as the
duplicate paragraph above describes for linked modules. Two foreign imports
that bind one local name to *different* modules (`import a as x`, then
`import b as x`) are refused like any other shadow, within one module and,
since [#1291](https://github.com/rotnov/pycc/issues/1291), across linked modules too. A name is reported once however many
statements bind it.
The positional binding above is what makes the artifact honest about *when*
the import runs; it is not enough to make the compiler honest about *which*
binding a name has, because every pass that walks the module would have to
reproduce the same positional rule. Export discovery is the one where that
became a wrong artifact rather than a wrong diagnostic: `collect_exports`
kept a `PyMethodDef` entry for a `def` that a later import supersedes, so a
host calling `compiled.<name>` reached the stale function where CPython hands
back a module object. Refusing the shape is one rule at one site
(`pycc_hir::import::reject_shadowed_foreign_imports`), it is fail-closed, and
it makes the same-module case agree with the cross-module one above.
Supporting either order is later work under #1026.

**Native and embedded mode.** A plain `pycc build` of a program with foreign
imports produces an embedded executable (see "Embedded executables" below),
which compiles the module exactly as `--ext` does; a root outside the
standard library bundles its closure from `pycc.lock` (#1242), and a missing
or stale lock is exit 2 naming `pycc lock`. A root imported only inside a
`try` whose handler catches `ImportError` is optional (#1290): it is locked
like any other root, but when no installed distribution owns it the lock
records no package, the build bundles nothing for it, and at run time the
import raises `ModuleNotFoundError` and the handler runs, as in CPython
without the package. The effective interop policy is
decided first, per import: a root it rejects is `I0402` on every host (see
"Interop policy" below). An import the build cannot embed (an excluded
Tcl/Tk root or a `--target` build) leaves a plain build with
no interpreter to import into, so the driver refuses the program with `I0403`
before codegen —
one diagnostic per such import, each at its own `import` statement in the file
that wrote it, with the reason ([D-248](./decisions/D-248-embedded-executable-artifact-layout-and-bridge-split.md)
rule 1) — and `crates/pycc_codegen/src/foreign_import.rs` emits nothing for a
`MirItem::ForeignImport` when `!options.ext`, which then only happens in a
build with no foreign import at all. A block-level `MirStmt::ForeignImport`
(#1291) needs no such guard: the same driver refusal means it reaches codegen
only in a build compiled with `ext` set, `--ext` or embedded, and an embedded
build runs it exactly as `--ext` does (`tests/issue_1291_block_import.rs`).

#### Embedded executables (Part 1 of #1028)

[D-248](./decisions/D-248-embedded-executable-artifact-layout-and-bridge-split.md)
owns the contract; this is the runtime view of it.

- **Artifact.** `OUT` plus `OUT.pycc/`: the embed interpreter's shared
  library under `lib/` (macOS: id `@rpath/libpython3.14.dylib`, ad-hoc
  re-signed), its standard library under `lib/python3.14/` without
  `site-packages`, `__pycache__`, `test` and the Tcl/Tk roots, and a
  `PYCC-BUNDLE` marker. A program importing a root outside the standard
  library also gets `closure/`: every file of each distribution its
  `pycc.lock` section names, copied from the locked site directory and
  checked against the lock (#1242, D-249 rule 7); a standard-library-only
  program has no `closure/`. The executable finds the library through an rpath
  relative to itself, so the pair is relocatable together.
- **Execution.** `src/embed/pycc_embed_launcher.c` starts an isolated
  interpreter (`home` is the sidecar, `platlibdir` is `lib` whatever the
  build host's was, no `site`, no bytecode writes, unbuffered stdio), creates `__main__` from the compiled module's
  `PyModuleDef` and runs it with `PyModule_ExecDef`; the module body runs
  with the GIL held. The compiled module is the one `--ext` would build,
  through the same unchanged C shim, with export thunks suppressed; no
  function is exported.
- **Isolation.** `sys.path` holds only the bundle's entries -- the bundled
  standard library, then `closure/` when the build created it (appended
  after `Py_InitializeFromConfig`); `PYTHONPATH` and ambient
  `site-packages` are ignored.
- **Exit status.** An uncaught exception prints through `PyErr_Print` and
  exits 1; `sys.exit(n)` exits `n`; a failed finalization exits 120; on a
  Windows host, a stub that cannot load its program DLL prints
  `pycc: cannot load <path> (error N)` to stderr and exits 121 (D-253).
- **Deviations from CPython.** `sys.flags.isolated` and `sys.flags.no_site`
  are 1, `sys.platlibdir` is `lib`, `sys.executable` and `sys.argv[0]` are the executable, and an
  uncaught pycc exception prints only its final `Type: message` line where a
  native build prints the whole chain.
- **Output ordering.** `buffered_stdio = 0` and `pycc_rt`'s flush at every
  newline keep Python-side and pycc-side writes in order; a future
  `print(..., end=...)` must flush before each foreign call.
- **Native libraries.** Every dependency of libpython, a `lib-dynload`
  module, a closure image or a copied library is resolved at build time:
  on macOS by its install name, on Linux as `ld.so` would on the build host
  (#1243). A system library is kept. A library under the interpreter's
  prefix, or (for a closure image) a native locked in `[[target.native]]`,
  is copied into `OUT.pycc/lib/`; on macOS the reference is rewritten to
  the copy, and on Linux the executable links every copied library by name,
  so the loader finds it already loaded when an extension module asks for
  it. An interpreter image needing anything else is refused as not
  relocatable. A Linux `DT_NEEDED` that resolves nowhere on the build host
  is left to the target's loader when an image loaded on import needs it,
  and refused when a copied library needs it, since the executable loads
  those at start-up. On Linux a closure image's dependency that resolves
  through `$ORIGIN` to another image of the closure's payload is kept.
  On macOS a closure image's or a native's `@rpath`/`@loader_path`
  dependency is resolved the same way from the image's source directory and
  its own `LC_RPATH` entries (#1259): a locked payload file is kept or
  rewritten to an explicit `@loader_path` path to its `closure/` copy, and
  anything outside the payload, the prefix and the system directories is a
  native; `@executable_path`, an unresolvable reference and any other form
  (neither absolute nor `@rpath`/`@loader_path`, such as a bare relative
  name) are refused. On both hosts a closure image's or a native's dependency
  under a site-packages directory is a native unless it is kept or rebound
  as a payload file, even when that directory lies inside the prefix or
  (on Linux) under a system library directory.
- **Windows host (Part 1 of #1226, #1286, #1296).** A program
  builds on a Windows host as a stub `OUT` plus `OUT.pycc\`
  ([D-253](./decisions/D-253-windows-embedded-executable-a-stub-out-loading-a.md)).
  The stub (`src/embed/pycc_embed_stub_windows.c`, static CRT, importing
  only the system DLLs `KERNEL32` and `ntdll`) loads `OUT.pycc\pycc_program.dll` with
  `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`, so
  the DLL's own imports resolve from the sidecar root and never from `PATH`,
  and calls its `pycc_embed_main`. The program DLL holds the launcher, the
  shim, the compiled module and `pycc_rt`, linked against `python314.dll`.
  The sidecar holds `python314.dll`, `python3.dll`, the interpreter's
  `vcruntime140.dll` and `vcruntime140_1.dll` when present, the filtered
  `Lib\` and `DLLs\` (without `site-packages`, `__pycache__`, `test` and the
  Tcl/Tk files), the program DLL and the marker, plus `closure\` holding
  the program's locked pure-Python closure when it imports a root outside
  the standard library (#1296); there is no `lib\`. The
  launcher sets `sys.path` explicitly to `<sidecar>\Lib` then
  `<sidecar>\DLLs`, then `<sidecar>\closure` when a closure is bundled, and
  leaves `platlibdir` at its default. The build links
  the program DLL into the swapped sidecar first and the stub at `OUT`
  second: a failed program-DLL link leaves a sidecar without the DLL (a
  stale `OUT` then exits 121), a failed stub link leaves a complete sidecar
  beside a stale or missing `OUT`; both are exit 1. Deviations and limits:
  the stub does not resolve symlinks; `sys.executable` and `sys.argv[0]` are
  the stub's path as spawned; Windows 10 or later is required; whether the
  artifact runs without the VC++ redistributable is not proven by CI (the
  runners install it); and relocation is not fail-closed -- `DLLs\` is
  copied wholesale with no PE import scan, so a `.pyd` depending on a library
  outside the sidecar and System32 fails only after the move, until #1297.
  A locked closure holding a file Windows would load as a PE image (a `.pyd`
  or `.dll` suffix, or an `MZ` header on any suffix other than `.exe`) is
  refused at exit 2 naming #1297 and `pycc build --ext`, and a static
  libpython is refused there (D-251).
- **Static libpython (Part 1 of #1227).** `pycc build --static-libpython`, or
  `[build] static = true` in a neighboring `pycc.toml`, links the embed
  interpreter's `LIBPL` archive into the executable whole (macOS
  `-force_load`, Linux `--whole-archive`) and exports its symbols (macOS
  `-export_dynamic`, Linux `--export-dynamic`), followed by the
  interpreter's `sysconfig` `LIBS` and `SYSLIBS`, so a `lib-dynload`
  module's C-API references resolve against the executable itself
  ([D-251](./decisions/D-251-static-libpython-link-for-embedded-executables.md)).
  The sidecar then holds no libpython; its marker records the archive's
  digest as `libpython-sha256` and adds `libpython-link static`. Any bundled
  image that needs a shared libpython -- by a `libpython3.` file name of any
  minor (so the stable-ABI `libpython3.so` too), a `Python.framework`
  binary, or a name that resolves to the interpreter's own library -- is
  refused at exit 2, since it would load a second interpreter into the
  process; a locked closure's extensions and natives included. A build
  that consumes a `pycc.lock` section checks the section's
  `libpython-sha256` against the file that identifies the interpreter, not
  the archive it links: the shared library when the interpreter is
  configured with one, else the archive, which is also what `pycc lock`
  records for it (Part 2 of #1227, #1272). A real archive is exercised
  end to end by `tests/issue_1273_real_static_archive.rs` (Part 3 of
  #1227, #1273) on the Linux CI legs, whose `actions/setup-python` CPython
  3.14.7 is built `--enable-shared` and still installs `LIBPL/libpython3.14.a`:
  the executables start CPython with no libpython in their sidecar or their
  dynamic dependencies and load `_json`, `math`, `_random` and `_ssl` from
  the bundled `lib-dynload`; one also bundles a locked closure. Their output
  matches that interpreter's, except the closure's file path. A host whose `LIBPL` holds no genuine archive gets
  the refusal instead.

A module body that fails reports through one of two channels, and the exec
slot preserves whichever one carries the failure. `pycc_rt`'s thread-local
pending state becomes a live CPython exception; a body that called into
CPython directly leaves an exception CPython already set, with no pycc
pending state at all. The generic `ImportError("pycc module body failed")`
is raised only when neither channel is set, so a real exception -- a
`ModuleNotFoundError` from a failed host import, say -- reaches the importer
unchanged instead of being replaced by a message that names neither the
cause nor the culprit.

pycc classifies each resolved import as a native pycc module or a
CPython-backed dependency. A CPython-backed import generates an interop bridge
without requiring a source rewrite to `pycc.interop`. The deployment artifact
bundles the pinned CPython 3.14 runtime, the resolved package artifacts, and
their native-library closure, so the target machine does not need a separately
installed Python or ambient `site-packages`. The resolver, the `pycc.lock`
schema and the closure's bundle layout are [D-249](./decisions/D-249-pycc-lock-schema-environment-resolver-and-update-command.md)
(`pycc lock` writes the lock and an embedded build consumes it, #1242); the embedded
interpreter must never search an unpinned ambient environment.

#### Interop policy

The build policy controls whether that automatic bridge is permitted. The
policy itself is implemented (#1224): `--interop-policy`, `--pure` and the
`[interop]` table select it, `check`, `build` and `run` enforce it, and a
rejected root is `I0402`. `docs/CLI_SPEC.md`'s `pycc.toml` section owns the
resolution and validation rules. What an admitted root then builds is
D-248's embedding: a standard-library root needs no lock, and any other
root is bundled with its closure from `pycc.lock` (#1242), or, when it is
an optional root (#1290) that is not installed, with none.

| Policy | Behavior |
|---|---|
| `auto` | Default. Permit every CPython-backed import root present in the source; an embedded build bundles its pinned dependency closure from `pycc.lock` (#1242). |
| `allowlist` | Permit only direct CPython-backed import roots listed in `[interop].allow`. Reject another direct root with `I0402`. An allowed root's pinned transitive closure is bundled with it without separate entries (#1242); a dotted CPython-backed import is `C0001` today. |
| `deny` | Reject every CPython-backed import with `I0402`. Native pycc modules remain available and the artifact has no CPython/libpython dependency. `--pure` is the CLI shorthand. |

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
