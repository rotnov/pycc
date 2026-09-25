# pycc Type System Specification

The contract: **surface syntax is standard Python typing** (PEP 484 → 695/696/742/649, full list in [PYTHON_STANDARDS.md](./PYTHON_STANDARDS.md)); **checking is strict** — what mypy calls `--strict` is pycc's only mode; **types drive codegen** — every check result is also a representation decision.

## Strictness rules

1. Every public function/method: parameters and return type annotated, else `T0001`.
2. Locals and private helpers: inferred (Hindley-Milner-flavored local inference; annotations always win).
3. `Any` does not exist in pure pycc code — it is a compile error (`T0002`) except at compiler-classified CPython interop boundaries (see RUNTIME.md § interop). Planned v0.7 creates that boundary behind ordinary standard-Python imports; it does not require a pycc-specific import spelling (D-128).
4. No implicit `Optional`, no implicit numeric narrowing **or widening** (D-086 — includes `int` at a `float`-annotated boundary; `float(x)` for `x: int | float | bool` is now a real callable builtin conversion, D-086's own remedy for that boundary, correctly deferring to a user-defined `float` of the same name if one exists — `int(...)` remains unimplemented and out of scope, and a bigint-valued `int` argument raises a catchable `OverflowError` since Part C of #1038 ([#1065](https://github.com/rotnov/pycc/issues/1065)), formerly a process abort -- the same pre-existing limitation every other numeric promotion to `float` already has, now reported rather than fatal), no untyped containers (`x = []` requires inferable or annotated element type).
5. Unreachable code after exhaustive `match` / `Never` is verified (`assert_never` pattern supported).
6. `==`/`!=` require operands to be comparable under the same numeric-like-or-`str` grouping ordering operators use (`int`/`float`/`bool` interchangeably, or `str`/`str`) — looser than the exact-type rule assignment/parameter/return boundaries enforce (rule 4), but still strict enough to reject genuinely incompatible pairs. Heterogeneous equality across categories (`1 == "1"`) is `T0021`, not `bool`, matching `mypy --strict`'s own `comparison-overlap` check (D-086). Ordering operators (`<`/`>`/`<=`/`>=`) use this identical grouping but are always rejected across it when CPython itself would raise `TypeError` at runtime for the pair.
7. Binary operators over `str` are operator-sensitive rather than decided by a single numeric-like grouping. `str + str` is concatenation and produces `str`; **`str * int` and `int * str` are repetition and produce `str`** (#574), in either operand order, with `bool` accepted as the count consistently with `bool <: int` (rule 4's no-implicit-widening rule governs *annotation boundaries*, not this operand grouping). Every other combination stays `T0021`: `str * float` and `float * str` (a repetition count must be integral), `str * str`, and any other operator over a `str` operand (`str - int`, `str / int`, and so on). The same rule applies identically in the validation pass and in private-helper constraint inference — both consume the one `numeric_result_type` helper. **Repetition now executes natively** (#575, Part 2 of [#123](https://github.com/rotnov/pycc/issues/123)): `pycc_mir` carries it as `BinOp { op: Mul, ty: Ty::Str }`, `pycc_codegen` lowers both operand orders to `pycc_rt_str_repeat`, and Part 1's D-072 exit-`101` boundary is gone from [CLI_SPEC.md](./CLI_SPEC.md) § Exit codes. A non-positive count yields the empty string, matching CPython (`"ab" * 0` and `"ab" * (0 - 2)` are both `""`); a negative *literal* count is reachable since [#602](https://github.com/rotnov/pycc/issues/602) folds a source-level sign into its literal, and a bigint count hits D-141's existing runtime int boundary. Since [#148](https://github.com/rotnov/pycc/issues/148) (D-178) that boundary is reachable from a source-level *literal* as well as from an arithmetic promotion: an `int` literal outside D-061's tagged 63-bit range now type-checks and compiles, materializing a heap bigint at run time, so `"ab" * 4611686018427387904` -- and every other bigint-valued word crossing an `int` boundary (container value, index, slice bound) -- reaches the runtime abort rather than failing during code generation. **Since [#618](https://github.com/rotnov/pycc/issues/618) (`T0051`)** an out-of-range literal written directly at one of these boundary positions is instead rejected pre-lowering by `pycc_hir`, restoring the compile-time catch D-178 gave up for the literal case; this is narrower than D-178's own boundary inventory for the `str * int` repeat-count position specifically, since `pycc_hir` has no type information yet at lowering time to tell a `str`-typed variable from any other operand -- `"ab" * 4611686018427387904` (a string *literal* on the left) is caught, but `s * 4611686018427387904` for a `str`-typed variable `s` is not, and still reaches the same run-time `pycc_rt_int_untag_checked` abort as before. An arithmetically promoted bigint (`s * (n + 1)`) is unaffected in every position, exactly as D-178 left it. `range` operands are no longer among them: [#147](https://github.com/rotnov/pycc/issues/147) (D-179) made `range()` bigint-capable in its bounds, its step, and an induction variable that promotes mid-loop, while keeping the D-074/D-141 bool-normalization contract unchanged (`range` consumes the numeric value and produces ordinary int objects rather than forwarding bool identity). The static type of such a literal is unchanged: it is `int`, exactly as before.
8. The shift and bitwise operators `<<`, `>>`, `&`, `|` and `^` ([#1210](https://github.com/rotnov/pycc/issues/1210), Part 2 of [#1018](https://github.com/rotnov/pycc/issues/1018)) are defined over `int` and `bool` operands only, and produce `int`, except that `&`, `|` and `^` of two `bool` operands produce `bool` (`True & False` is `False`, `True << 1` is `2`). Every other operand pair is `T0021`: "operator `|` on `set[int]` and `set[int]` is not supported yet" for the pairs CPython defines but pycc does not implement yet (`set` with `set` for `&`, `|` and `^`, and `dict | dict`), and "... is not defined" for every other pair (`1.0 << 1`, `"a" ^ 1`, `{1} | 1`). The operators are bigint-capable in both operands and in the shift count, with CPython's semantics: a negative count raises `ValueError: negative shift count`, and `>>` floors. One deviation: `<<` raises `OverflowError: too many digits in integer` wherever CPython raises `MemoryError`, which pycc has no class for -- a result too large to allocate, and a non-zero base shifted by a count from `2**62` up to CPython's own `OverflowError` bound (`1 << (1 << 62)`); the D-244 amendment of 2026-09-23 records it. Both operand orders and the unannotated-helper solver consume the same `numeric_result_type` helper, so an inferred `bool` argument to `def _g(a, b): return a & b` merges to `int` as for any other operator.

### v0.1 local inference

- A module-level function whose name starts with `_` is a private helper under
  D-038. Missing parameter and return annotations create inference variables;
  explicit annotations remain fixed constraints.
- **This scoping does not widen to methods**, deliberately, and
  [#1143](https://github.com/rotnov/pycc/issues/1143) does not change it.
  D-038's leading-underscore rule carries two separable jobs: a *visibility*
  predicate (`pycc_hir::is_public_name`) and this *inference* convention.
  #1143 widens only the first, applying the same unforked predicate to a class
  name and a method name so that a public `@staticmethod`/`@classmethod` of a
  public class joins the `ext` export set. The inference convention below stays
  module-level: a method body is not a private-helper inference root, a
  `_`-prefixed method creates no inference variables, and every method
  parameter and return still needs a written annotation. Widening the
  inference convention would be a separate change with its own solver work,
  not a corollary of the export-set widening.
- The v0.1 solver links those variables through call arguments, local names,
  assignments, returns, `range` operands, and arithmetic expressions. The
  resulting helper signature is monomorphic within the module. Conflicting
  call-site constraints are `T0021`; conflicting inferred returns are `T0022`.
  `T0022` also covers the unrelated-to-inference case in the same solver walk:
  a function with a *written* return annotation whose body returns an
  incompatible value. The two are worded differently -- an annotation is a
  declared/actual mismatch, an unannotated helper's return genuinely is an
  inference conflict -- and the return annotation, not the function's
  visibility, is what tells them apart (#949); the solver walks every
  module-level function, not only `_`-prefixed helpers.
- An initialized scalar-annotated local with no earlier representation binding
  binds its target to the declaration type, while the initializer keeps its
  independently inferred type and is checked directionally afterward. A
  compatible re-declaration retains the first representation binding.
  Scalar declaration bounds act only as deferred fallbacks for
  otherwise-unresolved initializer variables: all bounds for the same
  inference root are aggregated and the most-specific compatible type wins,
  so `bool` evidence is never widened to `int` merely because an annotation or
  helper body was visited first. A value-less annotation still creates no
  initialized binding; that separate state-model limitation is tracked by
  [#245](https://github.com/rotnov/pycc/issues/245).
  Non-scalar annotated targets receive local-only unresolved solver bindings;
  an inference root joined to one cannot resolve an otherwise-inferred private
  parameter or return as a container, even through hard call evidence. Existing
  container inference from explicit function-signature evidence is unchanged.
- An unconstrained parameter or return variable is rejected with `T0021` and
  an instruction to add an annotation. It never silently becomes `Any` or
  `None`; only a helper with no value-returning path infers `None`.
- D-146 (#239): the solver now carries `Ty::List` as a destructured element-
  type carrier for homogeneous scalar-element list literals. The `ListLiteral`
  arm produces `Some(Ok(Ty::List(...)))` when all elements share an exact-equal
  private-solver scalar type; the `Subscript` and `ListPop` arms destructure
  that carrier to extract the scalar element type for scalar return-type
  inference (e.g. `def _first(): xs = [1]; return xs[0]` infers `int`). The
  carrier is never unified — `unify_terms` and `merge_inferred_types` are
  unchanged — and `dict`/`set`/`tuple` remain in the scalar-only gap.
  A *resolved* empty list (`HirExpr::EmptyList`, below) produces the same
  destructured carrier under the same `is_private_solver_scalar` gate, so
  `xs = []; xs.append(1); return xs.pop()` infers exactly what the `xs = [1]`
  spelling infers; an element type outside that gate (a nested list, a
  `Ty::Param`) keeps the historical `Ok(None)`. A resolved empty dict
  (`HirExpr::EmptyDict`) stays opaque, matching the `DictLiteral` arm, which
  produces no term either.
- Function-local names are classified before the body is checked. Parameters
  are local from entry; every assignment target and `for` target anywhere in
  the implemented nested control-flow grammar is local throughout that
  function. A read before the local has been bound is `T0021` and never falls
  back to a same-named module global. A name with no local binding form may
  still resolve to a module global. Call targets follow the same lookup before
  builtin or function-registry resolution: an unbound local target is `T0021`,
  while a bound local or parameter from the current primitive subset cannot be
  called by falling through to a same-named function. A call to a known Python
  3.14 callable builtin that this compiler version does not implement (e.g.
  `ValueError("x")`, `Exception("msg")`, `int("5")`, `range(10)` as a
  standalone call) is classified as `C0001` (capability gap), not `T0021`
  (name-resolution failure) -- the builtin genuinely exists, this compiler
  just does not implement it yet (issue #142). User-defined functions always
  take priority: a `def ValueError(...)` is called correctly, not classified
  as `C0001`. The same classification applies in both the final validation
  pass and the private-helper inference path. Definite-assignment
  tracking (issue #118 Part 1, D-147) extends this to control-flow joins: a
  name assigned in only one branch of an `if`/`elif` (no `else`), or only in a
  `while`/`for` body (which may execute zero times), is *maybe* bound after the
  construct. Reading a maybe-bound name is `T0041` (possibly-unbound read),
  distinct from `T0021` (never bound). An unconditional assignment on the
  current path after the join upgrades a maybe-bound name back to definitely
  bound, so `if c: x = 1` followed by `x = 2` makes `x` readable. After a
  `try` or `try`/`except*` statement (#1289), a name is definitely bound when
  every path that can complete the statement normally binds it: the `else`
  path after a completed body, plus every handler whose body does not always
  return or raise. `finally` is checked against the conservative state,
  because it also runs on the paths that leave early,
  so a read of a try-bound name inside `finally` is still `T0041`. Every
  path's binding is type-checked in `check_assignment`'s direction, including
  a handler that always terminates (a mismatch is `T0023`), and the name keeps
  the first path's type. A name that any handler of the statement binds with
  `except ... as` is left out of both rules on every path: its type and its
  definiteness after the statement are exactly the pre-#1289 conservative
  join's. So `try: e = 10 // d / except ZeroDivisionError as e:` is
  accepted, a later `e = 5` is `T0023` against the exception type, and a
  later read of `e` is `T0041` -- even when its handler always terminates and
  every other path binds it. That is a documented pycc limitation: CPython
  runs `try: e = 10 // d / except ZeroDivisionError as e: raise / return e`,
  but pycc gives `e` one storage slot typed for the exception instance, which
  the body's `int` cannot share, so it refuses the program rather than
  miscompiling it. This join deliberately does not reuse
  `join_if_branches`: that function checks the reversed direction, so it
  admits a later `int` into an earlier `bool` and keeps the `bool` -- which
  is why `if d == 0: x = True / else: x = 1 / print(x)` prints `True` for
  `d = 1` where CPython prints `1` (observed on this revision; an `if`/`else`
  defect outside #1289's scope) -- and it keeps a `Definitely` side's type
  over a `Maybe` one. The
  private-helper constraint solver mirrors this tracking (issue #118 Part 2,
  #359): its `ConstraintEnvironment` carries a `maybe_bindings` side-table
  populated by `join_if_branches_solver`/`join_loop_body_solver` (and, for
  a `try`, cleared for the names every fall-through path binds by
  `promote_try_fallthrough`), and
  `collect_expr_constraints`'s `Name` arm skips unification for maybe-bound
  names so the solver does not infer a private-helper return type from a value
  that may not exist (the validation pass's `T0041` remains the user-facing
  gate when an explicit annotation makes inference unnecessary). This
  contract governs every read, the one position an artifact-owned buffer name
  is admitted as a whole value included: the `memoryview` egress in the row
  below requires the returned name to be `Definitely` bound, because its
  artifact-owned provenance is tracked in a set that joins as a *union* and
  would otherwise admit a buffer allocated on only one path reaching the
  `return`. That egress carries a second, lexical precondition which is not a
  binding-state question at all and is stated in the row below: the function
  must contain no `return` statement inside any `finally` body.
- The first assignment fixes a local variable's inferred type. Later
  assignments must be compatible or produce `T0023`; an empty `[]`/`{}`
  re-assignment is the one shape that reads the *existing* binding rather
  than fixing a new type (see "Empty container literals" below), and it
  must keep that binding's representation, not merely its `Ty`; assigning `bool` to an
  `int` binding preserves the static `int` representation and, per D-141, the
  source object's runtime `False`/`True` identity. D-074/D-141 carry that
  decision through MIR and code generation at assignment, argument, return,
  container-value, instance-attribute-store, and `range` boundaries; range
  consumes the numeric value and produces ordinary int objects rather than
  forwarding bool identity. The instance-attribute store joined that list in
  [#627](https://github.com/rotnov/pycc/issues/627) (D-187): `pycc_mir`
  widens the value through `MirExpr::IntBoundary` when the slot's declared
  type is `int` and the value's is `bool`, so `c.n = True; print(c.n)` prints
  `True`. A `bool`-**declared** slot is left alone and keeps storing its raw
  word.
- Python numeric semantics apply during inference: `bool` is an `int`
  subtype, mixed `int`/`float` arithmetic promotes to `float`, and true
  division `/` always returns `float` even for two integer operands.
- A value-less annotation (`x: int` with no `= ...`) declares `x`'s static
  type without binding a value: a premature read is still `T0021`, exactly as
  an ordinary unannotated local would be. The declared type is retained and
  becomes the sticky representation the first time a later plain or
  annotated assignment reaches `x` — incompatible with that declared type is
  `T0026`, distinct from `T0023`'s "previously inferred" wording, since
  nothing was ever assigned, only declared. A repeated value-less
  declaration for the same still-unassigned name is itself checked against
  the first one (first-declaration-wins) and rejected with `T0026` on
  mismatch (issue #245). This general-checker rule is independent of the
  private-helper solver's own separate, still-open limitation described
  above.

### Empty container literals (#1021, [D-245](./decisions/D-245-resolve-empty-container-element-types-in-a-pre-check-hir-pass.md))

An empty `[]` or `{}` carries no element type of its own, and this checker has
no bidirectional inference to hand one down from context: `infer_expr_in` takes
no expected type. Before #1021 every empty container literal was an error,
including the annotated `xs: list[int] = []` form, because `AnnAssign` infers
its value before comparing it with the annotation.

A single infallible HIR-to-HIR pass (`pycc_types::empty_container`) now runs at
the top of *both* entry points — `check_all_keyed` (`pycc check`) and
`check_and_resolve_all_keyed` (`pycc build`) — before any checking, and
rewrites each resolvable empty literal into a typed
`HirExpr::EmptyList(Ty)` / `HirExpr::EmptyDict(Box<(Ty, Ty)>)` node carrying a
fully concrete element type. Running before both entry points is what makes
totality structural: `pycc check` cannot accept a program `pycc build` then
panics on. The element type has to reach HIR (and from there MIR) because
`pycc_mir` derives a container's type independently, from the literal's first
element, and panics on an empty one.

- **Three sources, in priority order.** (1) The annotation on an `AnnAssign`
  target — purely syntactic, so this path needs no environment and always
  works. (2) *Any* successfully-inferred binding for that name anywhere in the
  enclosing function, not only one that precedes the empty literal: the
  environment is built by one whole-function forward pass that completes
  *before* any rewriting, so `xs = [1]; ...; xs = []` and
  `xs = []; xs = [1]; ...` both resolve from `list[int]`. (3) A forward scan of
  the enclosing function body for the first *producer* use. Sources (2) and (3)
  are best-effort on a rebuilt environment and degrade silently rather than
  failing.
- **Why order-insensitivity is safe.** The pass resolves; it never accepts.
  `check_container_ty` admits only `list[int]` and `dict[str, int]`, so a
  resolution is either the one element type the program could have compiled
  with or a `T0034`/`T0036` — and the check phase then re-validates the whole
  function in true program order, including D-040's sticky-representation rule.
  A restriction to strictly-prior bindings would only turn some compilable
  programs into `T0003`; it could not turn an accepted program into a
  different one. That argument holds only while the rebuilt environment records
  the *same* representation the checker will: the binder must apply D-040
  stickiness on every rebinding form, a value-less `v: bool` after `v = 1`
  included, or a resolution can name a type the program never produces and the
  re-validation reports it as a real `T0034`.
- **Producers, not consumers.** Only `xs.append(v)` and `d[k] = v` supply an
  element type. `for x in xs`, `xs[0]`, `len(xs)` and `xs.pop()` *read* a type
  that must already be known; in a single forward pass with no backward
  unification they cannot produce one.
- **Producers in statement position only.** `xs.append(v)` is also a valid
  *expression* (`y = xs.append(v)` binds `None`), and that form does not
  resolve an empty literal — `xs = []` whose only `append` is a value-position
  one reports `T0003`. The restriction mirrors the rewrite side, which visits
  direct assignment values and block bodies and never descends into nested
  expression positions.
- **Every block statement is walked.** Both halves of the pass — the rewrite
  and the producer scan — share one inventory of nested statement sequences
  covering `if`/`else`, `while`, both `for` forms, every `match` case body, and
  every `try`/`except`/`except*`/`else`/`finally` suite, so source (1) — purely
  syntactic, reading no environment — behaves identically at any nesting depth.
  Sources (2) and (3) share one exception, and it is a narrowing rather than a
  contract: both read the same flat whole-function environment built by a
  pre-existing pass shared with protocol monomorphization whose own statement
  walk has no `match`/`try` arm. Source (2) reads a binding out of it directly;
  source (3) infers the producer's value *in* it. So a name bound only inside
  one of those suites is invisible to both. The cost is a missed resolution,
  never a wrong element type. What a miss reports is `T0003` in a `try` suite,
  but inside a `match` case the failing concrete path routes the module into
  the private-helper constraint solver, whose own `match` arm never binds
  pattern captures, and D-220's solver-first merge surfaces that pre-existing
  false `T0021` instead — see issue #1046; `main` emits the identical message
  for the same program and for a `match` program with no empty container at
  all.
- **First-wins within a scope.** When two branches assign `[]` to the same
  name with different producers, both nodes take the first producer's type and
  the second branch's `append` reports the ordinary element-type mismatch. One
  binding cannot hold two element types.
- **The scan stops at the first *syntactic* producer, inferring or not.** A
  producer statement that names the target but whose value does not infer ends
  the scan with a miss, rather than falling through to a later producer. The
  flat whole-function binder has no `match`/`try` arm, so a value bound inside
  such a suite is invisible to it; letting the scan continue would resolve the
  container from a producer the program's own first use contradicts — for
  `try: v = 1; xs = []; xs.append(v); xs.append(True)` that was `list[bool]`,
  reported as `T0034`, while the `xs = [v]` spelling compiles. The stop
  propagates out of nested bodies, so a producer after the block cannot select
  a type either, and both halves of a `d[k] = v` producer count: either the key
  or the value failing to infer is the same match. The cost is once more a
  `T0003`.
- **A name bound in more than one place is not evidence.** A producer may read
  a name its own branch or loop body binds, which the flat whole-function
  binder demotes and the scan restores per body — but only when that name has a
  single syntactic binding site in the whole function. With two sites the type
  the flat binder recorded may have come from a *mutually exclusive* branch:
  for `if flag: v = True` / `else: v = 1; xs = []; xs.append(v)` it is `bool`,
  which resolved `list[bool]` and reported `T0034` inside the `else`, while the
  `xs = [v]` spelling sees the branch-local `int` and compiles. Reconstructing
  each body's own bindings would reimplement the checker's statement walk ahead
  of it, so the evidence is declined instead, and two sites carrying the *same*
  type are declined with it. The cost is again a `T0003`; issue #1058 tracks
  narrowing that coarseness for loop targets, whose type is `int` at every site
  without consulting any environment.
- **The same gate as a written annotation.** A resolved type still passes
  through `pycc_hir::check_container_ty` (D-228), so an inferred `list[str]` is
  `T0034` and an inferred `dict[int, int]` is `T0036`, exactly as the written
  annotations are.
- **Only a fully concrete type is ever stored.** A resolution containing
  `Ty::Infer` anywhere inside it is discarded and the literal is left alone.
  `Ty::Infer` is the private-helper constraint solver's placeholder, and this
  pass runs *before* that solver, so a producer whose value is an unannotated
  helper parameter (`def _f(x): xs = []; xs.append(x)`) infers the placeholder
  here. Nothing downstream substitutes into a rewritten node, so storing it
  would report a `T0034` naming `list[<inferred>]` — a type the source never
  mentions — for a program whose `xs = [x]` spelling the solver accepts. The
  cost of declining is a `T0003`: a missed resolution, which this pass permits,
  rather than a wrong one, which it does not. `Ty::Param` is deliberately not
  discarded — both spellings of a generic element already report the same
  `T0034`, so there is no asymmetry to repair there.
- **The two inferred sources decline a narrowable type.** A resolution
  containing `Ty::Optional` anywhere inside it is discarded on the binding and
  producer sources — but *not* on the annotation source. Both inferred sources
  read the flat whole-function environment, whose narrowing overlay is empty,
  while the check phase resolves the same name inside a narrowed branch.
  Narrowing here is exclusively `Optional` narrowing (`narrow.rs` recognizes
  only a `name is None` / `name is not None` test against an `Optional`
  binding), so an `Optional`-carrying inferred resolution is exactly the set
  this pass can get wrong: for `if x is not None: xs = []; xs.append(x)` the
  flat environment yields `Optional[int]`, and storing it reports a `T0034`
  naming `list[int | None]` for a program whose `xs = [x]` spelling compiles
  and runs. The cost of declining is again a `T0003`. A written
  `xs: list[int | None] = []` keeps its `T0034`: the type is stated in source,
  no narrowing is involved, and that diagnostic names the real D-105 gap where
  a miss would be the worse answer.
- **The producer source declines a maybe-bound name.** The flat whole-function
  binder records every assignment target as definitely bound, including one
  assigned only inside an `if` branch or a loop body; the check phase joins
  such a name back as *maybe* bound and reports `T0041` on a read. A producer
  whose value is such a name would therefore resolve an element type out of a
  binding the checker itself refuses to read, and the resulting `T0034` would
  mask the `T0041` the `xs = [v]` spelling reports for the same program. The
  pass demotes those bindings before the scan, re-promoting per nested body the
  construct's own loop target and that body's top-level definite names -- a
  name is definite inside the body that binds it and only maybe-bound after it.
  A demoted binding makes `Environment::lookup` return `None`, so the producer
  source declines itself with no separate expression walk. The binding source
  reads the assignment target's own recorded type rather than a value
  expression, and is deliberately unaffected. The cost is once more a `T0003`,
  and the definite set is deliberately under-approximated: this pass can only
  decline to resolve, it can never make the checker report a different code.
- **No set path.** A set binding can only originate from an empty set literal,
  `{}` parses as a dict, and `set()` is rejected at HIR lowering with `C0001` —
  so `SetAdd` is a structurally dead producer and `set[T]` is untouched here.
- **What is still an error, now `T0003`.** Any position with no inferable
  element type: a call argument (`f([])`), a `return []`, a nested literal
  (`[[]]`, `{"k": []}`), a module-level assignment, and a local whose only
  later uses are consumers. Since [#1265](https://github.com/rotnov/pycc/issues/1265)
  it also covers an unannotated `self.xs = []` in `__init__` whose slot no
  source types: neither an inherited concrete slot of the same shape nor a
  `self.xs.append(v)` in one of the class's own methods. The message names
  the attribute and the class. An unannotated `self.d = {}` establishing
  the attribute in `__init__` and a tuple-unpacking target
  (`L, R = [], []`) are *not* part of this: both are rejected earlier with
  `C0001`, the first until its `self.d[k] = v` producer exists
  ([#891](https://github.com/rotnov/pycc/issues/891)) or a class-body
  declaration (`d: dict[str, int]`, [#1266](https://github.com/rotnov/pycc/issues/1266))
  types it. Under a declaration, an establishing `[]`/`{}` of the wrong shape
  for the declared slot is this `T0003`. A later reset
  `self.<attr> = {}` in a method is typed from the slot when the slot is a
  `dict`, and is this `T0003` when it is not. An empty tuple
  `()` and `set()` are unchanged too: they still report `T0021` (citing #927)
  and `C0001` respectively. The message names the binding only when the failing
  empty literal *is* the directly-assigned value (`xs = []`, `d = {}`); a
  `T0003` raised from a nested element position such as `xs: list[int] = [[]]`
  keeps the generic wording, because the unresolvable node there is the inner
  `[]` and not the validly-annotated outer binding. The substitution is wired
  into the function-scope assignment seams only, so a module-level `xs = []`
  keeps the generic wording even though it *is* a directly-assigned value.
  `T0003` was registered for exactly this meaning and never emitted before
  #1021.
- **The help text depends on the position.** `T0003` carries a `help` (visible
  on the `--error-format json` surface, and repeated by `pycc explain T0003`)
  chosen from `Environment::in_function_body`: inside a function body it asks
  for an annotation on the binding or a use that fixes the element type, while
  at module level it says neither works -- the pass does not run there at all,
  so `x: list[int] = []` reports this same code -- and points at moving the
  binding into a function body. A single wording would be circular at one of
  the two positions.

## Types and representations

| Python type | Static semantics | Native representation |
|---|---|---|
| `int` | arbitrary precision (CPython-true); accepts `bool` as a subtype at checked boundaries | one int-compatible `i64`: odd smallint, exact `2`/`6` bool-identity marker, or aligned heap-bigint pointer — see D-061/D-141 |
| `float` | IEEE 754 double | `f64`, unboxed |
| `bool` | subtype of `int` | standalone `i8`, unboxed; exact `2`/`6` marker only after crossing an `int` boundary; `i1` is transient control-flow state only — see D-061/D-074/D-141 |
| `str` | immutable Unicode | UTF-8 heap, small-string opt — see D-007 |
| `bytes` / `bytearray` | per CPython | raw buffer |
| `None` | unit | LLVM `void` for returns; canonical `i8 0` carrier for the v0.1 user-function parameter ABI plus parameter, local-assignment, and module-assignment storage; the MIR/static `Ty::None` tag keeps that carrier distinct from `False`; `T \| None` = nullable/tagged repr |
| `tuple[A, B]` | fixed heterogeneous | inline struct (stack when non-escaping) |
| `list[T]` / `set[T]` / `dict[K, V]` | homogeneous, invariant | native vec / swiss-table (insertion-ordered dict) |
| `class` | nominal | struct; fields fixed at compile time (`__slots__` semantics implicit) |
| `Protocol` | structural | compile-time-only interface; no runtime vtable or protocol object. **Current state (#380, PR-20):** `class P(Protocol):` with method declarations (`...`/`pass` bodies) and attribute annotations is implemented. Structural conformance is checked at compile time when a concrete class is assigned to a protocol-typed variable or passed to a protocol-typed parameter. A protocol class in *return-annotation* position (`def make() -> P:`) is rejected with `C0001` ([#934](https://github.com/rotnov/pycc/issues/934)): a call to such a function has no concrete type to bind, and D-166 gives a protocol no runtime representation to dispatch through. That includes a protocol member returning its *own* protocol, in either spelling -- `def clone(self) -> P: ...` or `def clone(self) -> Self: ...` inside `class P(Protocol)` ([#948](https://github.com/rotnov/pycc/issues/948)); a self-referential *parameter* or *attribute* is accepted and carries the protocol type. An attribute member is satisfied by an instance attribute, a `@property`, or -- since [#914](https://github.com/rotnov/pycc/issues/914) -- a class-level attribute, with the same type check in all three cases. `@runtime_checkable` enables compile-time `isinstance` against a protocol (presence-only structural check, over the same three attribute sources). Protocol inheritance (`class Q(P):` where `P` is a protocol) is supported. Protocol-typed variables and function parameters use monomorphization — the concrete type is bound for MIR dispatch. `abc.ABC` and `@abstractmethod` are compile-time-only markers. Generic protocols, `issubclass` with protocols, and runtime dispatch are not supported. |
| unions `A \| B` | tagged | discriminant + payload; niche optimization for `T \| None`. **Current state (D-197, #763, Part 1 of #747; widened by #809, Part 3 of #747):** only `T \| None` (PEP 604, either operand order) is recognized, and only for `T` in `{int, float, bool}` — `Optional[int]`, `Optional[float]`, `Optional[bool]`. A general union `A \| B` with neither side `None`, or any longer `A \| B \| None` chain, is rejected pre-lowering (`T0048`); a recognized `T \| None` shape with `T` outside `{int, float, bool}` (e.g. `str`, a container, or another `Optional[...]`) is rejected with `T0049`. The runtime representation is an explicit `{ payload, present: i8 }` struct passed and stored by value — `payload` is `i64` (D-141 encoded) for `Ty::Int`, plain `f64` for `Ty::Float`, and plain `i8` for `Ty::Bool` (not yet niche-packed into `payload` itself for any inner type, unlike the aspirational "niche optimization" description above — see `crates/pycc_codegen/src/lib.rs`'s `Scalar::Optional`). `Optional[bool]`'s real `{i8, i8}` shape happens to be the same anonymous LLVM struct type as the bare-`None` placeholder's own fixed `{i8, i8}` shape (LLVM literal struct types are uniqued per-field-type-list) — this is a harmless representational coincidence, not a bug: every `coerce_scalar_to_type` call site discriminates on the requested `Ty`, never on introspecting the `StructValue`'s LLVM type alone. `is`/`is not` against a literal `None` operand reads the `present` field as a boolean presence test; general object-identity `is`/`is not` between two arbitrary non-`None` operands remains unimplemented (`C0001`). **Current state (D-205, #769, Part 2 of #747):** a top-level `is`/`is not None` presence test now flow-narrows the value itself — see "Narrowing & flow typing" below for the full scope. |
| `Callable[...]` | first-class functions | fn pointer / closure struct |
| `enum.Enum` | per CPython | integer or string discriminant + const table. **Current state (#379, PR-19; widened by #892):** scoped `class C(Enum):` with `int`- or `str`-literal member values is implemented, as is `class C(StrEnum):`, whose members must all be `str`. Members are compile-time singletons (`Ty::Instance(C)`); `C.MEMBER.value` returns the member's own literal, typed per class (`int` for an integer-valued enum, `str` for a string-valued one); `C.MEMBER.name` returns the member name as a `str`, and both are read-only: a store to either (`c.value = ...`, `C.MEMBER.name = ...`, `c.value += 1`) is `T0044`, as CPython raises `AttributeError`; an augmented store through a computed base (`Color.RED.value += 1`) is refused earlier, as `C0001`, by the augmented-assignment target gate ([#1219](https://github.com/rotnov/pycc/issues/1219)); and `for c in C:` iterates members in declaration order (unrolled before MIR). `auto()` is supported in its bare-name, zero-argument spelling inside an enum body: the next integer in a plain `Enum`, the lower-cased member name in a `StrEnum`. Mixing `int` and `str` members in one class, non-literal or non-`int`/`str` values, duplicate names, multiple bases, generic enums, and method definitions in enum bodies are rejected with `C0001`. A member named after the instantiation or class-creation protocol (`__init__`, `__new__`, `__init_subclass__`) is rejected with `C0001` too ([#975](https://github.com/rotnov/pycc/issues/975), [D-236](./decisions/D-236-reject-a-class-attribute-named-after-the-instantiation.md)): an enum body is one of four routes to a class-level binding, and `class C(Enum): __init__ = 1` compiled here while CPython raised `TypeError: 'int' object is not callable` at class creation. `__new__` diverges the same way; the `__init_subclass__` rejection is *conservative* rather than a measured divergence, because an enum with members cannot be subclassed at all and CPython accepts `class C(Enum): __init_subclass__ = 1; B = 2`. `__slots__` in an enum body is rejected with `C0001` too, and is conservative on the same footing: CPython 3.13.9 accepts both `__slots__ = "x"` and `__slots__ = ()` there — `enum.py`'s `_EnumDict` keeps a dunder out of the member list, so `C.__slots__` is an ordinary class attribute and `list(C)` is unchanged — while an enum lowered here has no `__init__` and no instance layout for `__slots__` to declare, so the diagnostic gives the enum route its own wording instead of borrowing the plain-class one about a layout fixed from `__init__` ([#978](https://github.com/rotnov/pycc/pull/978)). "Conservative" there means only that CPython runs the program: without the guard pycc lowers the name as a member, so `__slots__ = "x"` alongside `A = "y"` would have compiled to a two-member enum where CPython has one. Since [#979](https://github.com/rotnov/pycc/issues/979) ([D-238](./decisions/D-238-reject-enum-body-assignments-cpython-keeps-out-of-the.md)) an enum body also rejects every assignment CPython's `enum._EnumDict.__setitem__` keeps *out* of the member list, in all four of its shapes: dunder-shaped `__x__`, name-mangled private `__x`, a name already spelled `_C__x` inside `class C`, and sunder-shaped `_x_`, each with its own `C0001` message. All four were measured divergences at `edc454ba` against CPython 3.13.9, not conservatism: `class C(Enum): __repr__ = 1; B = 2` iterated two members here and one there (`C.__repr__.value` printed `1` where CPython raises `AttributeError`), `__x = 1; B = 2` did the same while not being a dunder at all, `_order_ = 'B'; B = 'b'` did the same again, `_C__x = 1; B = 2` did the same while nothing mangled it, and `_foo_ = 1; B = 2` compiled here while CPython raised `ValueError` at class creation. `_is_private` needs two of the four shapes because it matches the *raw* dict key against the literal `_<ClassName>__` prefix: `__x` reaches it already mangled, while `_C__x` reaches it unchanged and matches anyway — so that one shape is class-name-keyed, and the same spelling in `class D(Enum)` stays an ordinary member. The rest of the rejection is a *shape*, so it is a deliberate superset of CPython's own set and over-rejects four families, all recorded in D-238: every sunder-shaped name, including the ten `_EnumDict` allowlists (`_order_`, `_ignore_`, `_missing_`, `_generate_next_value_` and their siblings, plus any `_repr_`-prefixed name), none of whose behaviors this compiler models; names matching only the `__`-prefix-and-suffix shape while failing `_is_dunder`'s `len > 4` and inner-underscore conditions (`__`, `___`, `____`, `___x___` are ordinary members under CPython); names with one leading underscore and two or more trailing ones (`_x__`, `_foo___`, `_C__x__`), which `_is_sunder`'s `name[-2] != '_'` condition excludes; and a `__x` assignment in a class whose own name begins with an underscore, where CPython's mangling strips the class name's leading underscores while `_is_private` does not, so `class _C(Enum): __x = 1` beside `B = 2` gives two members (`['_C__x', 'B']`) — modelling that would need a name-mangling pass, since pycc would otherwise report the member's `.name` as `"__x"`. `_x = 1` and `_foo = 1` are not in the set and stay ordinary members, agreeing with CPython. `__order__` is the one dunder CPython does not even leave as a class attribute — `_EnumDict` rewrites its key to `_order_` and `EnumType.__new__` pops it out of the class dict entirely. The guard is enum-route-only: in a plain or `@dataclass` body every one of those names is an ordinary class attribute under both engines and is still accepted. Calling an enum class (`C()`, `C(1)`) is rejected with `C0001` too — members are reached by name, and by-value lookup is not implemented ([#921](https://github.com/rotnov/pycc/issues/921)); an enum class has no `__init__` in its MRO by design, so `HirClassDef.is_enum` (set only by `lower_enum_class`, and `true` even for a member-less docstring-only enum) is what `resolve_instantiation` keys the rejection on (the reporting site is `pycc_hir`'s per-item scan at the call expression for every call it can attribute, [#944](https://github.com/rotnov/pycc/issues/944); the guard is the span-less backstop behind it for the scan's documented residuals). Naming an enum class as a base of an ordinary class (`class Foo(C): pass`) is rejected with `C0001` on the class header at HIR lowering ([#941](https://github.com/rotnov/pycc/issues/941)): an enum with members cannot be extended in CPython either (`TypeError: <enum 'Foo'> cannot extend <enum 'C'>`, which the message names), and extending a member-less docstring-only enum, which CPython allows, is not supported yet -- so a subclass of an enum is never lowered and never reaches D-225's constructor synthesis. `enum.Enum`, `enum.StrEnum`, and `enum.auto` all resolve as imports as well as bare names; `enum.IntEnum`, `Flag`, and `IntFlag` do not. Comparing two members (`C.RED == C.GREEN`) is not yet supported ([#908](https://github.com/rotnov/pycc/issues/908)), and neither is comparing a `StrEnum` member with a bare `str` — pycc has no `str` methods or `str`-mixin machinery at all. |
| `object` | opaque CPython value; not spellable in an annotation | an owned `PyObject *` (an LLVM pointer): `pycc_ext_obj_import` hands back the *new* reference `PyImport_ImportModule` returns, and the artifact retains it for its whole lifetime; an attribute load (`pycc_ext_obj_getattr`), a method call (`pycc_ext_obj_call`), a subscript load (`pycc_ext_obj_getitem`, which additionally *consumes* the packed key reference on every path, exactly as a method call's argument slots do) and both halves of a `for` loop -- the iterator (`pycc_ext_obj_get_iter`) and each item it yields (`pycc_ext_obj_iter_next`) -- each hand back a new reference that is likewise never released, while `len` (`pycc_ext_obj_len`) and a truth test (`pycc_ext_obj_truthy`) produce no reference at all and leave the operand's refcount untouched, and each Part 4 conversion (`pycc_ext_obj_to_float`, `pycc_ext_obj_to_int`, `pycc_ext_obj_to_str`; `bool(o)` reuses `pycc_ext_obj_truthy`) and PR 4c's tuple unpack (`pycc_ext_obj_unpack_float_tuple`, which writes plain `double`s through an out-param) releases the temporary its CPython protocol hands it on *every* exit, including the failing one -- for `pycc_ext_obj_to_str` the `pycc_rt_str_from_literal` copy must additionally complete *before* that release, because `PyUnicode_AsUTF8AndSize` points into the temporary's own buffer -- so no reference escapes and Part 4 adds nothing to the leak -- [RUNTIME.md](./RUNTIME.md) owns that ownership rule, including why the leak is trip-count-linear rather than once per process and what that means for D-244 rule 6 benchmarking. **Current state (Parts 1, 2, 3a, 3b, 3c, 4a, 4b and 4c of [#1026](https://github.com/rotnov/pycc/issues/1026), [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 3):** the first way to obtain one is a *foreign import* -- a plain, undotted `import <name>`, or since [#1291](https://github.com/rotnov/pycc/issues/1291) its aliased form `import <name> as <alias>`, whose root is neither a project module nor a [`pycc_std`](./RUNTIME.md) registration. That statement binds the module object itself, to `<alias>` when it has one. Since [#1278](https://github.com/rotnov/pycc/issues/1278) an unaliased, top-level `from <name> import a, b` of such a module is a foreign import as well: each listed name binds the CPython object `<name>.<a>` (`pycc_ext_obj_import_from`, whose new reference is retained the same way), so `from itertools import product` holds CPython's own `itertools.product` as an `object`. Calling that object directly (`product(xs, ys)`) is still the `I0404` "calling the object itself" refusal below. Since [#1291](https://github.com/rotnov/pycc/issues/1291) it may also stand inside a module-level `if` or `try` block, where it binds only if control reaches it: the block's ordinary definite-assignment join decides a later read, so a name both arms of an `if`/`else` import is definitely bound after the block, and a name only one arm (or a `try` body) imports is the ordinary `T0041` possibly-unbound read ([#1289](https://github.com/rotnov/pycc/issues/1289) tracks admitting those shapes). Part 2 adds the two remaining producers -- an *attribute load* on a value of this type (`numpy.pi`) and a *method call* on it with positional `int`/`float`/`bool`/`str` arguments (`numpy.sqrt(2.0)`), each of whose results is another `object`, because pycc knows nothing about the attribute's or the return's real type either; a call to an unannotated private helper whose inferred return is `object` is the same producer reached through the constraint solver. `object` remains deliberately **not** spellable in a source annotation (an annotation naming it is `C0001`, unchanged -- see [D-137](./decisions/D-137-stdlib-imports-bind-module-qualified-names-via-a.md)'s 2026-09-14 amendment), so the binding is reachable only through the import statement. Reading such a value is no longer itself an error -- Part 2 moved the refusal from the producer to each *consumer*, because the checker is context-free and cannot tell an attribute load's base from a `print` operand. That relaxation is bounded by **position**, and only a *module body below the import* is admitted. A read inside a function body is `I0404`: D-041 checks a function body against the module environment as it stands after all top-level code, so the checker cannot tell whether the call site runs before or after the `import`, and the compiler's failure path for a failed attribute lookup (returning `-1` from the `Py_mod_exec` slot) exists only in the module body. A module-body read placed *above* its own `import` is an ordinary `T0021` ("name `numpy` is not defined"), which is what CPython's `NameError` justifies. Both shapes were compile errors under Part 1, briefly type-checked under Part 2, and then died at run time with `SIGTRAP` on the global-initialization failure edge; PR 2a of [#1081](https://github.com/rotnov/pycc/pull/1093) restored the compile-time refusals. Lifting it needs ordering-aware name resolution plus a function-level exception protocol, because only the module-body entry point has a failure edge to take; PR 2b of #1081 therefore did not lift it, and gets that protocol for free instead -- the one function a foreign call can appear in is the one function that can return `-1`. Attribute access, a positional scalar-argument method call, `len`, truth testing, a scalar-key subscript load, `for` iteration, the `float(o)`/`bool(o)`/`int(o)`/`str(o)` conversions and an annotated module-level assignment to a fixed-arity all-`float` `tuple` are the eleven operations implemented. `len(o)` type-checks to `int` and is answered at run time by `PyObject_Size` (`pycc_ext_obj_len`), so an operand with no `__len__` raises `TypeError` in the host rather than failing to compile; using `o` as an `if`/`while` test or a comprehension guard is answered by `PyObject_IsTrue` (`pycc_ext_obj_truthy`), which runs the operand's own `__bool__`/`__len__` and can therefore raise too. Both refusals were compile-time under Part 2 and are run-time from PR 3a of [#1082](https://github.com/rotnov/pycc/issues/1082) on. `not o` is **not** among them and stays `T0021`, which is the pre-existing unary-operator rule rather than a foreign-object one. An argument of any other type -- including another `object`, so `gc.set_debug(json.dumps)` -- is itself `I0404`, naming the type, because the boundary has no packer for it. `o[k]` reads an element and answers another `object`, answered at run time by `PyObject_GetItem` (`pycc_ext_obj_getitem`), so a missing key raises the host's own `KeyError`/`IndexError`/`TypeError` instead of failing to compile; it was `T0033` under Part 2 and is admitted from PR 3b of #1082 on. The **key** is restricted to the same four packable scalars a method argument is, for the same reason -- any other key type, including a second `object`, is `I0404` naming it. Only the **load** is implemented: a *store* (`o[k] = v`) is refused by HIR lowering with `C0001` and a *slice* (`o[a:b]`) keeps its `T0033`, so admitting the read widened neither. `for x in <iterable>:` iterates the object, binding `x` to another `object` for the loop body, and is admitted from PR 3c of #1082 on. Because HIR carries no types, the *iterable* is admitted by **shape**: exactly `for x in o.attr:` and `for x in o.method(...):`, the two shapes that can produce an object, each still refused by the type checker with `I0404` when the iterable turns out not to be one -- which is a cost as well as a bound: **every** attribute or attribute-call iterable now lowers to this node and draws whatever the checker reports about the receiver first, where `pycc_hir` previously refused all of them with its own `C0001`. Three shapes show the range -- `for x in C.value:` over an `int` class attribute reaches this arm and is the `I0404` above, naming `int`; `for x in xs.copy():` over a `list[int]` and `for k in d.keys():` over a `dict[str, int]` are `T0043` ("cannot call a method on `...`: it is not a class instance"), raised while the iterable is inferred and so ahead of this arm; and a receiver with its own pre-existing refusal reports that instead, so an `int`-keyed `d.keys()` is `T0036`. The loop **target** is bound with `env.bind` rather than through `check_assignment`, whose K1 guard refuses `Ty::Object` outright, but a target that already names a binding of some other type is refused with `T0023`: `pycc_codegen` allocates exactly one storage slot per name per function, so `x = 5` followed by `for x in o.attr:` has no representable lowering, and the mirror ordering already reported `T0023` from `check_assignment`. Every other iterable keeps the refusal it already had: a subscript iterable (`for x in o[k]:`) and every non-producer shape stay `C0001` at lowering, and a bare imported module stays `I0404` -- a module object is not iterable. Exhaustion is **not** a failure edge: `pycc_ext_obj_iter_next` is three-valued (`1` item, `0` clean exhaustion, `-1` error), so only a raising iterable returns `-1` from the `Py_mod_exec` slot, and the loop runs zero times over an empty iterable rather than failing. Every other consumer refuses, with `I0404` where the reason is "not implemented yet" -- printing or f-string interpolation, binding the value to a name -- including a PEP 572 walrus target, which reaches the same `check_assignment` guard ahead of the `T0050` the operand would otherwise draw -- `isinstance`, a `match` subject, and calling the object itself -- and with the pre-existing generic diagnostics where one already applied: arithmetic and comparison (`T0021`), unary operators (`T0021`), slicing (`T0033`), a subscript *store* target (`C0001`), a tuple element (`T0039`), and the export boundary (`C0003`). Admitting method calls did not admit the four D-105 String-keyed spellings `append`/`pop`/`get`/`add`: those are stolen ahead of the `Ty::Object` branch and stay `I0404` ([#1095](https://github.com/rotnov/pycc/issues/1095)). A read whose only consumer is an unannotated private helper's `return` still gets the concrete `object` term from the constraint solver, which runs before the check phase: without the term, signature materialization would report a `T0021` asking for an annotation the type cannot be spelled in, and with it the helper's return type resolves and the real `I0404` -- the in-function-read refusal above -- is what the user sees; **shadowing the name is refused outright**: a module in which any other top-level statement binds a foreign import's local name -- a `def`, a `class`, a `type` alias, a plain assignment, or a second `import` (including a second foreign import of the same name that binds a different module; since [#1291](https://github.com/rotnov/pycc/issues/1291) a repeated foreign import of the *same* module under the same name, such as `import numpy` in both arms of an `if`/`else`, is admitted), written *above* or *below* the import -- is rejected with `C0001` while lowering, which subsumes the rebinding case D-040's sticky-representation rule would otherwise report as `T0023`. The reason is the containment invariant above: with one name bound both foreign and non-foreign, its meaning depends on the position of each read, and every pass that walks the module -- the check pass, the constraint solver, MIR lowering, export discovery -- would have to reproduce that positional rule independently. Three review rounds on [#1080](https://github.com/rotnov/pycc/pull/1080) each found one more that did not, most seriously export discovery, which kept a `PyMethodDef` entry for a `def` a later import supersedes, so a host calling `compiled.<name>` reached the stale function where CPython hands back a module object. Refusing the shape is one rule at one site, it is fail-closed, and it matches what the cross-module case (below) already does; supporting either order is later work under #1026. A *function-local* name that happens to spell a foreign import is not shadowing at all -- it is an ordinary local with no foreign provenance: the per-body environment strips the module-level marking for every local name, so the local's own value is inferred normally rather than as `object`. A plain `pycc build` embeds CPython (Part 1 of [#1028](https://github.com/rotnov/pycc/issues/1028), [D-248](./decisions/D-248-embedded-executable-artifact-layout-and-bridge-split.md)), so these operations then run in the bundled interpreter exactly as under `--ext`: a standard-library root needs nothing more, and any other root is bundled with its dependency closure from the program's `pycc.lock`, a missing or stale lock being exit 2 naming `pycc lock` ([#1242](https://github.com/rotnov/pycc/issues/1242), [D-249](./decisions/D-249-pycc-lock-schema-environment-resolver-and-update-command.md) rule 7). An import the build cannot embed (an excluded Tcl/Tk root or a `--target` build) is refused with `I0403`. In a multi-file program that refusal is reported against the file that actually wrote the `import`, which need not be the entry file named on the command line. Consequently the value has no drop and no reference-count adjustment, and the `ext` export boundary refuses an `object` parameter or return (`C0003`), so one can never escape the artifact. It does now have a **conversion path out**, from PR 4a of [#1083](https://github.com/rotnov/pycc/issues/1083) on for `float(o)` and `bool(o)` and from PR 4b on for `int(o)` and `str(o)`: each type-checks to the scalar it names and is answered at run time by CPython's own conversion protocol -- `PyNumber_Float` (`pycc_ext_obj_to_float`), `PyObject_IsTrue` (`pycc_ext_obj_truthy`, the helper the truth test above already uses), `PyNumber_Long` (`pycc_ext_obj_to_int`) and `PyObject_Str` (`pycc_ext_obj_to_str`) -- so an operand with no `__float__` raises the host's own `TypeError` rather than failing to compile, and a converted value is an ordinary scalar that may be printed, bound, or used in arithmetic. `int(o)` answers a D-141 encoded word and refuses a result outside pycc's inline-integer range `[-2**62, 2**62-1]` with `OverflowError`, exactly as the thunk boundary does and citing the same [#1040](https://github.com/rotnov/pycc/issues/1040); there is deliberately no bigint path across this boundary. `str(o)` copies CPython's UTF-8 into an ordinary pycc `str` with `pycc_rt_str_from_literal`, the identical call a `str` literal uses, so the result is indistinguishable downstream from a literal-derived one; a lone surrogate, which has no UTF-8 encoding, surfaces CPython's own `UnicodeEncodeError`. From PR 4c on there is also a **destructuring path out**: a module-level annotated assignment whose annotation is a fixed-arity all-`float` `tuple` (`v: tuple[float, float, float] = gc.get_threshold()`) binds an ordinary D-115/D-116 by-value tuple, admitted by a branch of the module-level `AnnAssign` check rather than by any widening of assignability -- `object` is still assignable to nothing. The rule is **strict container, converting elements**: `pycc_ext_obj_unpack_float_tuple` checks `PyTuple_Check` (not `CheckExact`, so a structseq such as `sys.version_info` is admitted) with exactly the declared arity, then converts each item with `PyNumber_Float`. The container half is strict because the destination struct's shape is fixed at compile time and no arity makes a differently sized sequence representable; the element half converts for the same explicit-conversion reason `float(o)` does, so the items are never type-checked and a non-numeric one surfaces CPython's own exception at import time (`float('final')`'s `ValueError`). The arity is a parameter of the helper, never a constant, so every arity from one upward is admitted alike. A mixed annotation keeps its unchanged `T0025`, PEP 585's variadic `tuple[float, ...]` is refused earlier still at annotation lowering (`T0053`) because it has no arity to declare, and the same statement inside a function body keeps the in-function-read `I0404` above. The admitted pair is also still refused at every other declared position -- a `return` of the object under such an annotation is `T0022`, and the `ext` export boundary keeps its `C0003`. Neither is the thunk boundary's unpacker: `pycc_ext_unpack_int_at`'s `PyBool_Check`/`PyLong_Check` and `pycc_ext_unpack_str`'s `PyUnicode_Check` exist because that seam is closed, and refusing a duck type is exactly what an explicit conversion must not do. That `print(float(o))` type-checks while `print(o)` stays `I0404` is the intended reading: the refusal is on the object, not on a value derived from one. **Running a conversion protocol here is not a [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 7 violation.** Rule 7 keeps the type boundary closed at the *thunk export seam*, where a value crosses implicitly and its annotation is the whole contract, so no protocol may run behind the author's back. `float(o)` in user source is an *explicit conversion request*: the author named the destination type, so running the operand's own `__float__`/`__index__`/string parse is precisely what was asked for. The relaxation is `object`-only and the residual incoherence is stated rather than hidden: `bool(o)` compiles while `bool(1)` is still `C0001`, because Part 4 relaxes exactly the object case and leaves the general builtin-conversion story to [#1017](https://github.com/rotnov/pycc/issues/1017)/[#1018](https://github.com/rotnov/pycc/issues/1018). The same holds for the other two: `str(o)` compiles while `str(1)` is still `C0001`, and `print(str(o))` type-checks while `print(o)` stays `I0404` -- the string-conversion refusal is on the object, not on a `str` derived from one. A user-defined `def float`/`def bool`/`def int`/`def str` still wins over the corresponding builtin, as it did before, and so does a user-defined `class float`/`class bool`/`class int`/`class str`: MIR resolves a call naming a class as an instantiation, so each arm consults the class table before admitting an `object` argument and the program keeps the refusal it had on `main` -- `float(o)` its own `T0021` argument-type message, and `bool(o)`/`int(o)`/`str(o)` the shadowing constructor's own parameter check. `float`'s class guard is confined to its `Ty::Object` admission because that arm already admitted `int`/`float`/`bool`; the other three arms admit nothing but `object`, so their guard sits on the whole arm and the two placements are equivalent. The pre-existing divergence for a *non*-object argument to a shadowing class (`class float` plus `float(3)` type-checks and then panics in codegen, because the builtin arm answers ahead of the class table) is untouched here and tracked by [#1107](https://github.com/rotnov/pycc/issues/1107). The `float` argument-type refusal keeps its wording unchanged and deliberately does not enumerate `object`: the type is unspellable in an annotation, so "pass an `object`" would be advice nobody can act on. The binding also does not cross a *module* boundary yet: re-exporting another project module's foreign import (`from dep import numpy`) and defining a top-level name that a different linked module binds as a foreign import are both refused with `C0001` while lowering -- [RUNTIME.md](./RUNTIME.md)'s "The bound name does not cross a module boundary yet." owns that rule and the reason (the binding is positional in its own module's item list). |
| `memoryview` (also spellable `ndarray` or `NDArray`) | a one-dimensional `float64` buffer, spellable in an annotation at a **parameter** position of a [`pycc build --ext`](./RUNTIME.md) artifact, and -- since Part 2b of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1164](https://github.com/rotnov/pycc/issues/1164)) -- at the **return** position of a public one, which [#1174](https://github.com/rotnov/pycc/issues/1174) widened from a module-level `def` to any export, a public method of a public class included. The parameter's storage is borrowed from the host for one call; a returned buffer's is artifact-owned and handed to the host over a refcounted exporter. Three source spellings, one type: `ndarray` ([#1129](https://github.com/rotnov/pycc/issues/1129)) and `NDArray` ([#1134](https://github.com/rotnov/pycc/issues/1134)) are each recognized as a bare name with no import and lower to the same `Ty`, so every rule in this row applies to all three and every diagnostic renders the canonical `memoryview` -- see D-244's #1129 amendment, statements (b) and (c), for why one type rather than several. Both of those spellings are resolved **after** a program's own classes and type aliases, so a module-level `class ndarray` or `type NDArray = ...` keeps its own meaning and the carrier is only what an otherwise-unbound name falls back to: unlike `memoryview`, `int` or `str`, they are ordinary identifiers rather than builtins, and reserving one ahead of a user's own binding would mean something Python does not (statement (h)). Registering `NDArray` does **not** on its own make a `numpy.typing`-style annotation compile: `from numpy.typing import NDArray` is refused by the foreign-import path, `import numpy as np` has bound `np` as a foreign import since [#1291](https://github.com/rotnov/pycc/issues/1291) but spells no annotation by itself, and an attribute-form base `np.ndarray` is [#889](https://github.com/rotnov/pycc/issues/889) | a `{ ptr, len }` pair (an LLVM pointer to a two-word POD the generated wrapper owns on its own stack), never CPython's `Py_buffer` and never the owning `PyObject *`. **Current state (Part 2 of [#1027](https://github.com/rotnov/pycc/issues/1027) and [#1116](https://github.com/rotnov/pycc/issues/1116), [D-244](./decisions/D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 7):** the annotation lowers, the `ext` boundary acquires, checks and releases the buffer, and exactly three operations on the value are implemented. `b[i]` is a bounds-checked load of one `float` element (`pycc_rt_buffer_f64_get`), whose index must be an `int` (`T0021` otherwise, and `bool` is accepted for it under rule 4/D-086) and is never wrapped when negative (D-108: `b[-1]` raises `IndexError`, it does not read the last element). `len(b)` is the buffer's element count as an `int` (`pycc_rt_buffer_len`, the wrapper's copy of `shape[0]`, in elements and never in bytes, re-tagged per D-141); it cannot fail, and it makes `for i in range(len(b)):` -- a one-call sweep of the whole input -- expressible. `b[i] = v` (Part 1 of [#1142](https://github.com/rotnov/pycc/issues/1142)) is the bounds-checked store of one element (`pycc_rt_buffer_f64_set`), with the load's index rule and a value that must be a `float` -- `T0021` otherwise, since rule 4/D-086 grants no int-to-float widening, so `b[0] = 1` is refused rather than converted. The two reads are answered by *interception* in `infer_expr_in` and in the constraint solver, before the name is read; the store is answered where `HirStmt::DictSet` is checked, by dispatching on the target's resolved type ahead of the `dict` requirement, so every other store target keeps its existing path and its `T0033`. Every *other* read of the name the parameter binds is still a `C0001` capability gap, which is what closes that set rather than enumerating it: `memoryview` has no literal and no producing expression, so iteration over the name itself (`for x in b`), slicing, aliasing into a local, storing the buffer into a container and passing it to another function are each reachable only through that read and are all refused by it at once. Returning one is the exception, and splits by provenance since Part 2b of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1164](https://github.com/rotnov/pycc/issues/1164)): a bare name holding *artifact-owned* storage may be returned from a public `--ext` export -- a module-level `def` or, since [#1174](https://github.com/rotnov/pycc/issues/1174), a public method of a public class -- provided that name is **definitely assigned** at the `return` -- the owned-buffer set joins as a union across control flow while the binding joins on the `Definitely`/`Maybe`/unbound lattice stated in the definite-assignment clause above, so `if c: a = ndarray(4)` followed by `return a` is that clause's ordinary `T0041` possibly-unbound read and not an egress (D-244's round-3 amendment of Part 2b; the condition is stated over the *result* of a join, so it closes `if`, `while`, `for`, `match` and `try` at once), and provided the function contains no `return` statement lexically inside any `finally` body, at any nesting depth -- `C0001` otherwise (D-244's round-5 amendment of Part 2b). That second condition is lexical rather than a property of the binding, and it is a precondition of the *code generator* rather than of the type: the compiled frame carries exactly one pending-return record, while a `return` inside a `finally` suspends the return whose exit path ran that `finally` and so puts a second one in flight, which used to compile into an artifact that segfaulted the hosting interpreter. It is computed once per function by the transitive `pycc_hir::body_returns_inside_finally` -- one helper both walkers consult, so the two cannot drift -- and it descends into every block under a `finally`, `while`/`for` bodies and nested `try` clauses included, because the front end's own PEP 765/D-193 `L0001` refusal of a bare `return` in a `finally` deliberately clears its context on loop entry (mirroring CPython) and therefore cannot serve as this check. [#1173](https://github.com/rotnov/pycc/issues/1173) tracks admitting the shape, which needs a stack of pending records rather than one. The refusal is gated at the egress admission only, so a `return` inside a `finally` in a function whose return type is not the buffer type compiles exactly as before -- while returning the name a `memoryview` **parameter** binds is the type's *second* admitted egress, and its second provenance at the return position, since Part 1 of [#1175](https://github.com/rotnov/pycc/issues/1175) ([#1178](https://github.com/rotnov/pycc/issues/1178)): a bare `return b` from a public `--ext` export hands the host a view over storage the **caller** owns. The refusal it replaces was stated with its reason -- the wrapper releases the host's buffer on the way out, so handing it back would be a use-after-free -- and that reason was false. The correction is mechanical rather than a relaxation of a safety rule: the generated wrapper acquires a second, independent buffer export on the argument object with `PyMemoryView_FromObject` **before** it releases its own `Py_buffer`, and an export -- not a reference -- is what pins an exporter's storage, so the returned view spans storage the caller's object still owns and the artifact allocates and frees nothing on that path (`pycc_rt_buffer_live_views` is zero for the whole call). The acquire-before-release order is load-bearing rather than stylistic, and only a PEP 688 Python-level exporter can observe it: between a release and a later acquire the host object stands at zero outstanding exports, which is exactly when an exporter that recycles storage on its last release would reclaim it. The returned view's writability is the host exporter's own, unlike the artifact-owned path's hard-coded `readonly = 0`. It carries the same preconditions as the artifact-owned egress -- definitely assigned, in a function with no `return` lexically inside any `finally`. Part 2 ([#1179](https://github.com/rotnov/pycc/issues/1179)) widens the admitted expression from the bare name to a **step-free sub-range of it**, `return b[start:stop]`, with each bound absent or `int`-typed. The bounds do not narrow the returned pointer: they travel out of the callee frame in three trailing out-slots and CPython derives the sub-view host-side, after the identity and second-export checks above have run over the whole window. Because that admitted branch is taken before the ordinary slice arm, it type-checks its own bounds, so a non-`int` bound is `T0021` and an unbound one `T0041`. A present `step` is `C0001` in every spelling including the identity `1`, with a message of its own naming the parameter: the buffer view carries no stride, so a strided sub-view is not representable. Still `C0001`: a slice of artifact-owned storage, a slice in any position other than the returned expression, and returning the result of an intra-artifact call that itself returns a buffer, because the wrapper decides which parameter a call returned by pointer identity against its own `args[i]` slots and a value produced in a callee frame need match none of them. Every *other* read of a parameter-bound name is still that same `C0001` read refusal, with its message unchanged. `return _make()` returns a *call's* result rather than a bound name, and a buffer-returning call is its own `C0001` in both walkers: the egress exists for the CPython host, and the artifact has no consumer for the value. `memoryview(x)` as a call stays `C0001` too. None of `memoryview`, `ndarray` and `NDArray` is added to `is_builtin_type_name`, so `isinstance(x, memoryview)`, `isinstance(x, ndarray)` and `isinstance(x, NDArray)` all stay refused: Part 1 admits the name in an annotation, and admitting it as a run-time class object as well would state a subtyping question the boundary never asks (D-244's #1129 amendment, statement (e)). That is what keeps the buffer's lifetime wholly wrapper-owned, which is the whole of the argument for releasing it in the wrapper rather than adding lifetime machinery to the compiler. [RUNTIME.md](./RUNTIME.md)'s `ext` admissibility matrix owns what the boundary accepts and refuses at run time; a `memoryview` **return** type is carried on a public `--ext` export -- since [#1174](https://github.com/rotnov/pycc/issues/1174) a public method of a public class as much as a module-level `def` -- and is a `C0001` capability gap on every function outside that export set (a private one, a specialization, a private method, a method of a private or exception class, an unreachable instance method, a `@property` getter), while an *intra-artifact call* to a buffer-returning method is a `C0001` at the call site, refused at all four method-resolution exits by `pycc_types`' `buffer::refuse_buffer_returning_method` -- and a `memoryview` in a *signature* -- a parameter or the return type -- in a build without `--ext` is `I0405`. A signature was the only position the type was admitted at through Part 1: every non-signature position is a `C0001` capability gap in both modes, for the same reason reading the name is -- a declaration (`x: memoryview`, at module scope or in a function body) and a protocol attribute are refused here, and a class attribute and a dataclass field by the scalar-slot restriction (D-154) that already governs them (a value-less class-body declaration, since #1266, by its own admitted-type gate). **Part 2a of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1165](https://github.com/rotnov/pycc/issues/1165))** adds exactly one more admitted position, and with it the type's second provenance: `a = ndarray(n)` -- or `NDArray(n)`, and deliberately *not* `memoryview(n)`, which is a `TypeError` in CPython rather than something pycc does differently -- allocates a zero-filled one-dimensional `float64` buffer of `n` elements that the artifact itself owns, admitted only as the whole right-hand side of an assignment, bare or annotated, to a local name inside a function body. The annotated spelling adds the ordinary annotated-assignment rule rather than an exception to it: the declared annotation must itself admit the buffer type, so `a: NDArray = ndarray(4)` binds artifact-owned storage while `a: int = ndarray(4)` is the ordinary `T0025` for an initializer that does not match its annotation. Both type walkers apply this clause's whole enumeration, and the constraint solver's must be a *subset* of the check phase's: the solver's answer for a function displaces the check phase's, so any condition missing from the solver's mirror shows up as a correct diagnostic replaced by the owned-buffer `C0001` rather than as an accepted program. Three consecutive review rounds of [#1165](https://github.com/rotnov/pycc/issues/1165) found such a missing condition in the solver's mirror: the stdlib module alias (round 6) and the foreign import (round 7), each recorded at its own clause below, and in round 8 the length type, together with three more that enumerating the check phase's whole set turned up -- the declared annotation above, PEP 591's `Final` reassignment refusal (`T0045`), and the parameter-rebinding refusal that was already mirrored. One member of the check phase's set is knowingly left unmirrored, because it changes no outcome: recording artifact-owned provenance for a name whose binding did not become a buffer (`a = 1` then `a = ndarray(4)`) leaves the later read off the owned path anyway, so the check phase's `T0023` still reaches the user. One residue is left open and is not a subset violation of the enumeration above but of its timing: a length term the solver cannot yet resolve (`a = ndarray(n)` on a helper's own unannotated parameter) is admitted at the seam, so a caller that later fixes `n` to a non-`int` reaches the `T0033` only if no use of the owned name in that body raises the `C0001` first. Every other call position is its own named `C0001`, raised from the `Call` arm of both walkers so that a bare `ndarray(4)` statement -- whose inferred type `check_stmt_in_function` would otherwise discard -- cannot leak one allocation per call; module scope is a separate `C0001`, because the free-at-function-exit lifetime has no frame there; and a length that is neither an `int` nor a `bool` is `T0033` -- a `bool` is an `int` by this table's own representation rule (rule 4/D-086), exactly as it is for the index above, so `ndarray(True)` is an admitted one-element request. Statement (h) governs the call position exactly as the annotation position, so a program's own `class ndarray`, `def NDArray`, module-level `ndarray = ...`, **stdlib module alias** (`import math as ndarray`), or **function-local** binding of the spelling anywhere in the calling body keeps its own meaning. A sixth binding kind is a non-stdlib `import ndarray` -- or, since [#1291](https://github.com/rotnov/pycc/issues/1291), `import X as ndarray` on a foreign module `X`, the same foreign binding -- whose outcome is a refusal rather than the program's own meaning and which is therefore stated with the other import forms below; this clause together with that sentence is the canonical enumeration of those binding kinds, and every other document cross-references it. The alias arm is the one whose absence was a *miscompile* rather than a misdirected message ([#1165](https://github.com/rotnov/pycc/issues/1165) review round 6): an alias binds the spelling in the import table and in no other, so `a = ndarray(4)` under `import math as ndarray` used to allocate a buffer silently under `--ext` for a call CPython answers with `TypeError: 'module' object is not callable`. It is now the program's own `T0021` (`call to undefined function `ndarray``) under `pycc check`, `--ext` and a native build alike, exactly as an aliased spelling that is not a producer (`import math as m`, then `m(4)`) already reported. The other two import forms stay refused upstream: `from math import sqrt as ndarray` is `C0001` (`from ... import x as y` aliasing is unimplemented) and a non-stdlib `import ndarray` or a foreign `import X as ndarray` is the foreign-import `I0404`. The `from ... import` form needs no arm of its own anywhere, because it is refused before any binding exists; the foreign form needs none in the checker, which binds the imported name as an ordinary opaque-object value, but does need one in the constraint solver, which deliberately keeps foreign names in a table of their own instead ([#1165](https://github.com/rotnov/pycc/issues/1165) review round 7). Its absence there was a misdirected message rather than a miscompile -- the program was refused in both cases, but the solver first marked the assigned name artifact-owned, so a second use of it reported the owned-buffer `C0001` at the `import` line instead of the `I0404` the foreign refusal owns. The local arm follows CPython's own scoping rather than the walk's position: a body that binds the name anywhere makes it local throughout, so an `ndarray(n)` earlier in that same body is that program's `UnboundLocalError` (`T0021`) and never a producer. A value-less annotation (`ndarray: int` with no `= ...`) splits on scope, because that local arm is CPython's own scoping rule rather than a test for a run-time binding, and this row is the canonical statement of the split. At **module** scope it binds nothing, so the producer stays in place, in both artifact modes. In a **function body** it is one of the binding forms the local-name pre-pass counts -- an `AnnAssign` is collected whether or not it carries a value -- so it makes the spelling local throughout that body, and an `ndarray(n)` anywhere in it is that program's `UnboundLocalError` (`T0021`) in both artifact modes and under `pycc check` alike, the native-mode body refusal included. That asymmetry is CPython's own `NameError`/`UnboundLocalError` split, not a pycc divergence. That single admitted position is what keeps the lifetime sound without lifetime machinery: the value cannot be aliased, stored or passed, so the allocating frame is provably its sole owner -- with the one transfer Part 2b of [#1142](https://github.com/rotnov/pycc/issues/1142) ([#1164](https://github.com/rotnov/pycc/issues/1164)) adds, a bare `return a` from a public `--ext` export, which hands ownership to the host-facing exporter. That transfer happens in the frame's owned-slot epilogue rather than at the `return` statement -- the epilogue releases every owned buffer slot whose pointer is not the value actually being returned -- because a `return` only makes the value pending and every interposed user finalizer runs before control leaves the frame. Because a `memoryview` binding now has two provenances, the refusals are keyed on the *binding* rather than the type: both environments carry an `owned_buffers` set, joined as a union at every branch, loop and match join (narrowed by the invalidation rule stated two sentences below), and using an owned name beyond the three implemented operations and the one admitted egress gets its own message, distinct from the parameter one, because the two answer different questions: an owned name is storage the allocating frame frees on exit, while a parameter-bound one is storage the host lends for the call and the artifact never owns. Assigning to a name bound to a buffer *parameter* is `C0001` for both of those reasons at once. Provenance is nonetheless not monotone, because reassigning an artifact-*owned* name is deliberately admitted: since #1166's round-11 review every binder other than the producer seam -- a plain or annotated assignment, a walrus, a `for` target, a comprehension target, a `match` capture, an `except ... as` name -- drops the name's owned marker together with its `memoryview` term, and each join additionally drops a name some joined path binds to something else, so a stale marker can no longer refuse a later read with the owned-buffer `C0001` over the check phase's own `T0023` for the reassignment. Reallocating into the same name (`a = ndarray(4)` then `a = ndarray(8)`) goes through the producer seam and stays owned, because codegen frees the previous allocation before the store (D-074). |

## Generics

- Monomorphization (Rust model): `list[int]` and `list[str]` are distinct compiled types; bounds expressed via `Protocol` constraints. **Current state (through PR-10, D-105):** this is the v1.0 target model, not yet fully landed — v0.2 ships real codegen for exactly `list[int]`; every other `list[T]` (`list[str]`, `list[float]`, `list[bool]`, nested `list[list[T]]`) type-checks (`Ty::List(Box<Ty>)` is already fully general) but is rejected before codegen with `T0034`, a clean diagnostic rather than a runtime panic. **Current state (through PR-11a, D-121/D-122):** `dict[str, int]` and `set[int]` now also ship real codegen (a dense, insertion-ordered array with linear-scan lookup — D-121 — not yet the swiss table this section's own v1.0 target describes); every other `dict`/`set` key/element combination type-checks (`Ty::Dict`/`Ty::Set` are already fully general per D-089) but is rejected before codegen with `T0036`/`T0038`, mirroring `list[int]`'s own `T0034` gate. **Current state (through PR-11b, D-115/D-116):** `tuple[...]` now also ships real codegen for exactly `int`/`bool`/`float` elements (any mix, any arity ≥ 1) — a fixed-arity LLVM struct-of-scalar-fields held as an SSA aggregate value, not a heap object (D-115); every other element type (`Ty::Str`, or any nested container) type-checks structurally (`Ty::Tuple(Box<Vec<Ty>>)` is already fully general) but is rejected before codegen with `T0039`, mirroring `T0034`/`T0036`/`T0038`. `t[k]` requires a literal, non-negative, in-range integer index (`T0040`) rather than merely an `int`-typed one, since a heterogeneous tuple's element type at position `k` is only knowable when `k` is known at compile time. `for x in t:` iteration and tuple-unpacking assignment (`a, b = t`) are deferred (`docs/ROADMAP.md`). A `tuple[...]` annotation syntax is **no longer deferred** -- see the annotation-surface bullet below (D-228). Passing or returning a tuple value across a function boundary is reachable from real source since [#925](https://github.com/rotnov/pycc/issues/925) (Part 2 of #918): a `-> tuple[...]` annotation lowers, and `pycc_codegen`'s call-result dispatch (`crates/pycc_codegen/src/call_result.rs`) carries a real arm for `Ty::List`/`Ty::Dict`/`Ty::Set`/`Ty::Tuple` rather than the panic D-116's correction note described. The annotation is what supplies the type: `pycc_types`' private-helper signature-inference solver is still scalar-only by construction, so an *unannotated* helper returning a container is still out of reach. Full per-element-type monomorphization for `list`/`dict`/`set`/`tuple` remains the v1.0 target this section describes.
- **Current state (through PR-13, D-133/D-134/D-135, and #387):** `def f[T](...) -> ...` now ships real codegen for exactly **one type parameter**, called from any number of call sites, each independently monomorphized by substituting `Ty::Param(Box<String>)` (D-133) with that call site's own concrete `Ty`. Instantiation is scalar-only, matching `pycc_types`' existing scalar-only signature-inference solver: `T` may resolve to `Ty::Int`/`Ty::Float`/`Ty::Bool`/`Ty::Str` only (D-134). Two or more type parameters, a type parameter used in a container position (`def f[T](x: list[T])`), a call-site argument type outside the four scalars above, inconsistent occurrences of `T` across a signature, and a generic function calling itself or another generic function (direct or mutual recursion) are all rejected pre-codegen with `T0042` rather than reaching codegen or panicking. The `type X = <expr>` statement and a legacy `X: TypeAlias = <expr>` assignment are compile-time-only name-to-`Ty` substitution inside `pycc_types`/`pycc_hir` — no `HirItem`/`MirItem`, no runtime representation, and no new `Ty` variant is produced for the alias itself (D-135); a generic alias (`type Alias[T] = list[T]`) is out of scope for v0.2 and is rejected pre-codegen with the same `T0042`. **[#387] PEP 695 generic classes** (`class C[T]:` with a single type parameter) extend this same monomorphization infrastructure: `C[int](args)` produces a specialized class `0gen_C__T_int` with substituted attribute types and method signatures, and `is_assignable` accepts `Ty::Param` as matching any concrete scalar during type checking. `Self` (PEP 673) resolves to `Ty::Instance(class_name)` at HIR-lowering time, and self-referential class-name annotations in own methods work (PEP 649/749); both spellings resolve to `Ty::Protocol(class_name)` instead when the enclosing class is itself a protocol ([#948](https://github.com/rotnov/pycc/issues/948), see the protocol section below). Subscripting a type parameter (`T[int]`) or `Self` in an annotation is rejected with `T0044` ([#931](https://github.com/rotnov/pycc/issues/931)); neither accepts a type argument. Multiple type parameters, container-position type parameters, generic aliases, and PEP 696 defaults all remain v1.0-target follow-ups (`docs/ROADMAP.md`), not yet implemented.
- PEP 695 syntax (`def f[T](x: T) -> T`, `type Alias[T] = ...`) and legacy `TypeVar` both supported; PEP 696 defaults honored. **This bullet describes the v1.0 target model, not current behavior** — see the v0.2 thin-slice paragraph immediately above for what actually ships today.
- Variance: inferred per PEP 695 rules; containers invariant, as in the typing spec.
- **Container annotation surface (through #918/#925, D-228).** `list[T]`, `set[T]`, `dict[K, V]` and `tuple[A, B, ...]` can now be *written* as annotations, not merely inferred from a literal. `pycc_hir::func::annotation_to_ty` lowers all four, so they are accepted in every position that routes through it: **function and method parameters, function and method return annotations (since [#925](https://github.com/rotnov/pycc/issues/925), Part 2 of #918), local and module-level `x: T = ...`, PEP 695 `type X = ...` and legacy `X: TypeAlias = ...` aliases**. Four rules apply, in this order:
  1. **An `...` type argument is rejected first, with `T0053`.** This is its own step ahead of the arity check, and the order is load-bearing: `tuple[int, ...]` has a legal arity of two, so an arity check alone would accept it. Scanning for the ellipsis first is what produces the variadic-specific message instead of silently lowering a `tuple[int, EllipsisType]`. The advice differs by family: only `tuple` is told to write a fixed-arity `tuple`, since `list[...]`/`set[...]`/`dict[str, ...]` are not variadic spellings at all and are told to write that family's element type instead.
  2. **Arity is checked next, also with `T0053`.** `list`/`set` take exactly one type argument, `dict` exactly two, `tuple` at least one. `T0053` here also rejects the empty `tuple[()]`, the other legal-Python spelling this version's fixed-arity `Ty::Tuple` cannot represent. Checking arity before element types keeps a wrong-arity annotation from reporting a misleading element-type error.
  3. **A `Ty::Param` type argument is `T0042`**, with the annotation's own span. `substitute_ty` is not recursive, so `def f[T](xs: list[T])` would never have its `T` substituted at a call site; rejecting it at lowering is what keeps that from becoming a silent miscompile.
  4. **The element restriction is unchanged and is now literally the same code.** A written annotation and an inferred literal share one gate (`pycc_hir::container`), so `list[int]`, `dict[str, int]`, `set[int]` and `int`/`bool`/`float` tuple elements are accepted, and everything else gets exactly the `T0034`/`T0036`/`T0038`/`T0039` it would have got from a literal -- now with a real caret at the annotation instead of the literal path's `1:1`. Since #1021 (D-245) an element type *inferred* for an empty `[]`/`{}` goes through that same gate too, so an inferred `list[str]` is the same `T0034` a written `list[str]` is.

  Two positions deliberately still reject a container type. (Return position was a third through Part 1 and is no longer: [#925](https://github.com/rotnov/pycc/issues/925) added the codegen call-result arms that the Part-1 `C0001` was standing in for, and removed the gate.) A **protocol attribute's** type gets `C0001`: a container-typed protocol attribute is not supported yet. D-228 first called it unsatisfiable, because every class attribute slot was scalar-only; since [#1262](https://github.com/rotnov/pycc/issues/1262) a hand-written `__init__` can hold a `list[int]`/`dict[str, int]` slot, so it is now unimplemented rather than unsatisfiable (D-228's 2026-09-24 amendment). A protocol *method's* parameter is unaffected and does lower — a parameter type is a signature type, not an instance slot. A PEP 695 **class type argument** and a `cast()` target still accept scalars only (`type_arg_name_to_ty`, `cast_target_ty`), a recorded inconsistency rather than a decision. A **bare** `list`/`set`/`dict`/`tuple` gets its own `C0001` naming the parameterized form to write — but only in a parameter, a return annotation, a local or module-level `AnnAssign`, or a type alias, the positions that actually lower one; an annotated attribute target (#1264) and a class-body instance attribute declaration (#1266) get it for a bare `list`/`dict` only, the two parameterized forms those positions lower. Every position that rejects the parameterized form (class constant, dataclass field, container element, protocol attribute, PEP 695 class type argument, `cast()` target) keeps the generic unknown-name message, so the advice never names a form that fails too. `frozenset` and `type` keep the generic message everywhere, since neither has a `Ty` variant at all.
- Code-size control: polymorphic-by-vtable fallback for cold generic code under `--opt-size` (compiler-internal, semantics unchanged).

### Protocols and structural typing (#380, PEP 544)

pycc implements PEP 544 protocols as **compile-time-only structural interfaces**. No runtime protocol object, vtable, or dynamic dispatch machinery is introduced — protocol conformance is checked statically, and protocol-typed values are dispatched via monomorphization (D-006, D-166).

**Protocol definition:** `class P(Protocol):` with method declarations (`...` or `pass` bodies only) and attribute annotations (`x: int`). A protocol method with an implementation body is rejected with `C0001`. A protocol attribute with a default value is rejected with `C0001`. `__init__` is not allowed in a protocol class. Generic protocol classes (`class P[T](Protocol):`) are not supported in v0.3.

**Protocol inheritance:** `class Q(P):` where `P` is a protocol creates a sub-protocol that inherits `P`'s protocol members. A class conforming to `Q` must satisfy both `P`'s and `Q`'s members.

**Structural conformance:** A concrete class `C` conforms to a protocol `P` if every required method (with matching parameter count, parameter types, and return type) and every required attribute (with a compatible type) exists through `C`'s MRO. Non-conformance is reported with `T0046`, identifying the concrete class, the protocol, and the missing or incompatible member.

**Protocol-typed variables and parameters:** When a value is assigned to a protocol-typed variable or passed to a protocol-typed parameter, the concrete (inferred) type is bound in the MIR — the protocol type is a compile-time-only annotation. This enables static dispatch through the concrete class's method table without any runtime vtable. A protocol class in **return-annotation position** is the one protocol position this version rejects: `def make() -> P:` (on a module-level function, a method, or a protocol member declaration) is `C0001` at HIR lowering, on the annotation's own span ([#934](https://github.com/rotnov/pycc/issues/934)), because a call to `make()` has no concrete type to bind and every shape of such a function used to type-check and then abort inside `pycc_mir` or `pycc_codegen`. A concrete function or method can annotate the concrete class it returns instead (`def make() -> C:`) and assign the result to a protocol-typed variable; a protocol *member* declared `-> P` has no workaround in this version. Since [#948](https://github.com/rotnov/pycc/issues/948) that gate also covers a member returning the protocol it is declared in, whether it is spelled with the protocol's own name (`def clone(self) -> P: ...`, PEP 649/749) or as `-> Self` (PEP 673): both resolve to `Ty::Protocol("P")` and reach the same `C0001` with the same message and the annotation's own span. A self-referential **parameter** (`def same(self, other: P) -> bool: ...`) and a self-referential **attribute** (`nxt: P`) are accepted on the protocol's own declaration and carry the protocol type, since `Ty::Protocol` is supported in those positions -- but the two differ in whether any concrete class can then conform. A concrete class satisfies the *parameter* member only by spelling the parameter as the protocol too (`def same(self, other: P) -> bool:`), because `check_protocol_conformance` matches member signatures by assignability rather than structurally: a concrete `other: C` is `T0046` exactly as a cross-class `other: D` against `other: Q` already is. Such a member is callable with a conforming concrete argument since [#953](https://github.com/rotnov/pycc/issues/953): `p.same(C())` and `c.same(C())` are accepted and run. Instance-method call arguments are checked with the environment-aware structural predicate (`is_assignable_env`), the same one `expr.rs` already used for plain function calls, so a class that conforms to `P` is accepted wherever `P` is expected; a class that does not conform still gets the unchanged `T0021`. Accepting the call requires the back end to follow: `monomorphize_protocol_params` now specializes `HirExpr::MethodCall` too, emitting a `0gen_{DefiningClass}.{method}__{P}_{C}` specialization (the class that defines the method, not the receiver's own class) exactly as it already did for plain calls, so the accepted program lowers and runs instead of aborting for want of a `$fn:` binding. The other call sites deliberately keep plain nominal assignability for now: a conforming concrete argument passed to a protocol-typed parameter of a `@staticmethod`, a `@classmethod`, a `super()`-forwarded method, a constructor (`binding.rs`) or an exception constructor is still rejected with the unchanged `T0021`, each verified against this version and pinned by a unit test (tracked as [#954](https://github.com/rotnov/pycc/issues/954)). The self-referential *attribute* member is unsatisfiable outright, for the same reason a container-typed protocol attribute is (above): every path by which a concrete class establishes an attribute slot restricts it to a scalar, so no class can declare an attribute of protocol or instance type at all. The check runs after the annotation lowers, so `-> list[P]` still reports `T0034` and `-> P | None` still `T0049`, and a PEP 695 type parameter named like a protocol (`def ident[P](x: P) -> P`) is unaffected.

**`@runtime_checkable` and `isinstance`:** A protocol decorated with `@runtime_checkable` can be used in `isinstance`. The check is evaluated at compile time as a structural conformance check (presence-only — method and attribute type compatibility is verified separately during static assignment/conformance checks). `isinstance` against a non-`@runtime_checkable` protocol is rejected with `C0001`. `@runtime_checkable` on a non-protocol class is rejected with `C0001`.

**`issubclass` with protocols:** `issubclass` involving a protocol as either the source or target is rejected with `C0001` — protocols use structural typing, not nominal inheritance.

**`abc.ABC` and `@abstractmethod`:** `ABC` is a compile-time-only marker base class (like `Enum` and `Protocol`). `@abstractmethod` marks a method as abstract — a concrete subclass that does not override every inherited abstract method is rejected with `C0001`. Instantiating a class with unoverridden abstract methods is rejected with `C0001`. An abstract method's body must be declaration-style (`...` or `pass`).

## Narrowing & flow typing

Flow-sensitive checker on HIR control-flow graph: `isinstance` (compile-time evaluated — see below), `is None`, truthiness of `Optional`, `match` patterns (PEP 634 — with exhaustiveness checking), `TypeGuard` (647), `TypeIs` (742), walrus bindings, `assert`. This whole section is the v1.0 target model; **current state (D-197, #763, Part 1 of #747; widened by #809, Part 3 of #747):** landed the representation `Optional[T]`/`T | None` needs to exist at all — the `Ty::Optional(Box<Ty>)` type (scoped to `T` in `{int, float, bool}`), its `{payload, i8}` runtime representation, and `is`/`is not` as a boolean presence test against a syntactic `None` operand — with no narrowing of the value itself yet. **Current state (D-205, #769, Part 2 of #747):** a *top-level* `if name is None:`/`if name is not None:` test (a bare `HirExpr::Compare` with one operand a bare name and the other a syntactic `None` literal — `pycc_hir::optional_none_test`) now narrows `name` from `Optional[T]` to plain `T` for the rest of the reachable region that test's polarity implies it must be present in:
- **In-branch narrowing:** `if name is not None: <body>` narrows `name` inside `body` only; `if name is None: ... else: <orelse>` narrows `name` inside `orelse` only (the mirror polarity). Implemented as a side-table overlay (`Environment.narrowed` at the checker layer; a `$narrowed:{name}` scope sentinel at the MIR layer) consulted only by name *reads*, never merged into the real binding table, so it cannot leak past the `if`/`else` join — see D-205 decision 1 for the full soundness argument.
- **The early-return continuation shape:** `if name is None: <body that definitely terminates>` (the body's last statement is a bare `return`, or an `if`/`else` whose both arms recursively terminate) narrows `name` for every statement *after* the `if`, in the same statement sequence — reaching that point is only possible via the implicit "`name` is not `None`" path. `definitely_terminates` (`pycc_hir::definitely_terminates`) is a new, strict predicate purpose-built for this — see D-205 decision 3 for why the pre-existing `contains_return` is unsound for this use.
- A reassignment of `name` anywhere inside a narrowed region kills the narrowing for every read after it, in that same region.
- **Not implemented:** narrowing through a compound `and`/`or` test or a test embedded in a larger boolean expression (only the bare top-level shape is recognized); narrowing a name *to* `None` (the mirror of the implemented shape — there is no `Ty::None`-typed narrowing target in this design); `raise` as an early-return-shape terminator (only `return` counts). See D-205's "Alternatives" section for the full rationale on each cut.

**Current state (D-206, #769, third D-068 re-review round of #780):** narrowing established by an enclosing `if`/`else` now also survives into `while`/`for` loop bodies (including the loop's own re-evaluated `test` expression), `try`/`except` handler bodies, and `match` case bodies — not just the straight-line/`if`-`else` shape D-199 covered. These constructs are *re-enterable*: a loop body runs more than once and a handler is entered only after some prefix of the `try` body already ran, so a statement earlier in source order can execute *after* a kill that appears later in source order. To stay sound under that, pycc runs a **kill-prescan** (`pycc_hir::killed_names`) before checking or lowering any such body: every name the body reassigns *anywhere* within it is dropped from the narrowing overlay for the *entire* body up front, rather than only from the kill's own position onward. This is conservative, not a fixpoint analysis — a body that narrows `name` on entry but kills it on iteration two loses the narrowing for `name` on *every* read in that body, including reads that would have been safe on the first iteration. A `try`'s `finally` body needs no separate prescan: it is checked against the environment produced by joining the try body's end-state with every handler's and the `orelse`'s end-state, and that join's own strict intersection already excludes any name killed on any of those paths. See [D-206](./decisions/D-206-kill-prescan-for-re-enterable-narrowed-bodies.md) for the full soundness argument, including the two counterexamples that motivated it.

`isinstance`/`issubclass` (#435, described below) predates this and is unrelated: it needs no flow narrowing because static dispatch already fixes every variable's declared type.

### Compile-time `isinstance`/`issubclass` (#435)

pycc uses static dispatch (D-006): every variable's runtime type is exactly its declared static type, so `isinstance` and `issubclass` are always evaluable at compile time. Both builtins are intercepted before generic call inference — the class argument(s) are class references (bare names or tuples of bare names), not value expressions, and are never lowered to runtime values.

- `isinstance(obj, cls)`: infers `obj`'s type, checks the class argument(s), and emits a compile-time `bool` constant. For user-defined classes, the result is `true` iff `cls` appears in `obj`'s class MRO. For builtin types, `bool` is a subtype of `int` (`isinstance(True, int)` is `True`); other cross-type checks are `False`.
- `issubclass(cls, target)`: both arguments are compile-time class references. For user-defined classes, the result is `true` iff `target` appears in `cls`'s MRO. `issubclass(bool, int)` is `True`; other cross-builtin checks are `False`.
- Tuple targets (`isinstance(x, (A, B))`): `true` if any member matches.
- Wrong argument counts, non-class arguments, and unknown class names are rejected with `T0021`/`T0001` before codegen.
- The MIR emits `MirExpr::BoolLiteral(result)` — no runtime RTTI, type tags, or calls.

## Annotation semantics

Per PEP 649/749 (3.14): annotations are lazily evaluated code — pycc evaluates them **statically at compile time**. A `from __future__ import annotations` file is accepted: the directive is a compile-time no-op that binds nothing ([#919](https://github.com/rotnov/pycc/issues/919), D-229), since static evaluation is already pycc's behavior. Three gaps remain explicit: a string annotation (`def f(n: "Node")`) is still `C0001` (tracked with attribute-qualified annotations by [#889](https://github.com/rotnov/pycc/issues/889)), a bare-name forward reference to a class defined *later* in the module is still `C0001` (``type annotation `Later` is not supported yet``) with or without the future import, and there is no runtime `__annotations__` object on functions, classes, or modules to introspect (dataclass field discovery reads the annotations statically at compile time instead, D-164) — all three recorded as `core` gaps for the PEP 563 row, flipped to `◐` by [#937](https://github.com/rotnov/pycc/issues/937).

Laziness is also why a **subscripted** annotation is not gated on the base being subscriptable. `Base[...]` in annotation position is accepted whenever `Base` resolves to a type pycc can name nominally — a known class, a `type` alias to one, or the buffer carrier in any of its three spellings, directly or through an alias — and the type argument is **erased without being lowered**, so an argument pycc could not lower at all (`NDArray[np.floating[Any]]`) is accepted along with the rest ([#1130](https://github.com/rotnov/pycc/issues/1130)). CPython 3.14 never raises for these: `class C: pass` followed by `def f(a: C[int])` raises nothing at definition time, and the `TypeError` surfaces only through `typing.get_type_hints`, which no pycc-compiled program can reach. **Value position is the opposite and is unchanged**: `x = C[int]` is evaluated eagerly and really does raise, so it keeps its own `T0044` ([#610](https://github.com/rotnov/pycc/issues/610)). That asymmetry is CPython's own. Three consequences are deliberate. (1) Accepting subscripts also makes the *bare-name* precedence ladder govern them, so a class shadowing one of the six names resolved before the class table (`int`, `float`, `bool`, `str`, `Any`, `memoryview`) now follows the reserved meaning in subscripted annotation position too. **That change runs in both directions, one per reserved name.** For the four builtin scalars, a subscripted shadow that compiles today (a PEP 695 generic or hook-bearing `class int[T]`) becomes `T0044`, and for `Any` it becomes `T0002` — the accept-to-reject half. For `memoryview` it goes the other way, because the carrier is itself a nameable type: a plain, hook-less `class memoryview: pass` followed by `def f(a: memoryview[float])` referenced *outside* that class's own body was ``error[T0044]: class `memoryview` does not define `__class_getitem__` `` on the branch point and is now accepted, with `memoryview[float]` resolving to the buffer carrier rather than to the user's class. That is not a new meaning: the *bare* `memoryview` annotation in the same program already resolved to the carrier before this change, so the subscripted form merely stops disagreeing with the bare one. No program silently changes meaning — a `memoryview` shadow that was already subscriptable (`class memoryview[T]`) resolved to the carrier both before and after. The sibling carrier spellings `ndarray` and `NDArray` are deliberately **not** on the reserved list — both are ordinary identifiers resolved *after* the class table ([#1129](https://github.com/rotnov/pycc/issues/1129), [#1134](https://github.com/rotnov/pycc/issues/1134)) — so `class ndarray: pass` plus `ndarray[float]`, and `class NDArray: pass` plus `NDArray[float]`, also flip `T0044` to accepted but resolve to the user's own class; that cell is consequence (3) below, not this one. (2) A subscripted class annotation on a **protocol attribute** moves from `T0044` to no diagnostic at all, since a protocol attribute typed by an instance is already legal. (3) `C[int]` where `C` is a genuinely non-generic user class is an ordinary typo that #611's gate used to catch and that pycc now accepts silently; that detection is forfeited, because keeping it would mean rejecting programs CPython runs.

### `Annotated[X, ...]` (PEP 593)

`Annotated[X, ...]` unwraps to `X` at HIR-lowering time — the inner type is resolved normally and all metadata arguments are discarded. `Annotated` is recognized as a bare name without requiring `from typing import Annotated`, matching the existing `TypeAlias`/`Any` precedent. No metadata-consuming behavior is implemented; the metadata is purely compile-time and has no runtime or type-checking effect.

### `Final[X]` (PEP 591)

`Final[X]` unwraps to `X` at HIR-lowering time — `Final` is a binding-level property, not a type-level one, so the unwrapped type is just `X`. `Final` is recognized as a bare name without requiring `from typing import Final`. A `Final`-annotated name may be assigned exactly once; any subsequent assignment (plain `x = ...` or annotated `x: Final[...] = ...`) is rejected with `T0045`. In the **variable-level** position the non-reassignability is tracked by the type checker's `Environment.finals` set, populated from `HirStmt::AnnAssign`'s `is_final` flag *after* the initial `check_assignment` call returns, so the initial binding is not rejected — only a subsequent reassignment. A value-less `Final` declaration (`x: Final[int]` with no `= ...`) also marks the name as `Final`; the first real assignment to it is allowed, but a second is rejected. Support covers variable-level annotated assignments (module-level variables and function-local annotated assignments) and, since [#916](https://github.com/rotnov/pycc/issues/916), **class-body attribute declarations**; `Final` on function parameters is out of scope. A class-body `Final` attribute takes a different route: it produces no `HirStmt::AnnAssign` at all, so `Environment.finals` never learns about it and `T0045` never fires for it — it is immutable because every write path to a class attribute is already rejected — `T0044` through an instance or `self`, `T0021` through the class name (see "Class-level attributes" below), which is exactly why the wrapper can be accepted without modelling a new guarantee. Both spellings are accepted there: `X: Final[int] = 1`, and a bare `X: Final = 1` whose type is inferred from the literal right-hand side. `Final` takes exactly one type argument in that position too, and the two PEP 591-invalid nestings — `ClassVar[Final[T]]` and `Final[ClassVar[T]]` — are each rejected with their own `C0001` naming the nesting. A redundant `Final[Final[T]]` is accepted and unwraps to `T`, exactly as it does in the variable-level position — the class body is not made a stricter special case, so a nesting rejection there names only the two PEP 591-invalid `ClassVar` combinations. `typing.Final[...]`, the attribute spelling, is not recognized in any position.

### Class-level attributes (PEP 526 / PEP 591's `ClassVar`, [#911](https://github.com/rotnov/pycc/issues/911), [#910](https://github.com/rotnov/pycc/issues/910))

A class-body assignment -- annotated (`X: int = 1`, or the explicit `X: ClassVar[int] = 1`) or un-annotated (`X = 1`, [#910](https://github.com/rotnov/pycc/issues/910)) -- declares a **class-level attribute**: a compile-time constant shared by the class and every instance. It is not a `HirClassDef` instance-attribute slot, takes no space in the allocation, never appears in `mro_attrs`/`mro_attr_count`, and produces no runtime storage at all — `pycc_mir` folds a read of it to its literal, so `pycc_codegen` and `pycc_rt` are unchanged by the feature. The one qualification, [#960](https://github.com/rotnov/pycc/issues/960): an *instance* read folds only when no instance slot of that name exists anywhere in the MRO, because a slot contributed by a sibling MRO base wins (see the collision bullet below). `ClassVar[T]` is registered as a `typing` annotation marker and stripped to `T` at HIR-lowering time, in the same shape as `Final`/`Annotated`; it is valid **only** in that position, and a `ClassVar` appearing in a parameter, return, or variable annotation is rejected with `C0001`.

Because the value is folded, the accepted surface is deliberately narrow:

- The attribute's type must be a **scalar slot type** — `int`, `float`, `bool`, or `str`. The annotated spelling resolves it from the annotation; the un-annotated spelling infers it from the literal, which can only ever yield one of those four, so the restriction holds there by construction rather than by a check. A class instance, `None`, a container, a protocol, or a generic type parameter (`Ty::Param`) is rejected with `C0001`. This is the same D-154 storage constraint the dataclass fields obey, and it is additionally the invariant that keeps `__set_name__` untriggerable (see the class-model note below): a descriptor is not a scalar, so a descriptor-valued class attribute cannot exist.
- The right-hand side must be a literal, optionally preceded by a unary `+`/`-` on a numeric literal. An `int` literal widens under a `float` annotation; every other mismatch, a complex literal (`1j`), a non-arithmetic unary operator (`~1`, `not True`), an `int` outside `i64`, a call, a name, or any other expression is rejected with `C0001`. A value-less `ClassVar` or `Final` declaration (`X: ClassVar[int]`) is rejected too — there is nothing to fold. A plain value-less annotation (`X: int`) is not a class attribute at all but an instance attribute declaration ([#1266](https://github.com/rotnov/pycc/issues/1266); see the class "Current state" paragraph). The un-annotated spelling additionally rejects a multi-target assignment (`a = b = 1`) and a non-bare-name target (`a, b = 1, 2`, `C.x = 1`).
- The name must not collide with anything else the class or its MRO already provides: another class attribute declared in the same class body (in either spelling, in either order), an instance attribute assigned in any `__init__` in the MRO, a `@property`, or a method, `@staticmethod`, or `@classmethod` — the latter three on the class itself or on any MRO base. A class attribute inherited from a base is *not* a collision: `lookup_class_attr_through_mro` resolves most-derived-first, so a derived class re-declaring a base's class attribute shadows it, exactly as CPython does. The check runs after the whole class body is walked, because the instance-attribute table is not complete until `__init__` has been reached. The method tables were added to it by [#910](https://github.com/rotnov/pycc/issues/910): a class attribute shadowing a same-named method diverges from CPython in both directions (a same-class collision makes CPython's later binding win, so it reads back as a bound method; a derived attribute over a base method makes the call `TypeError: 'int' object is not callable`), and neither is modellable while the read folds to a constant. What is deliberately **not** a collision is a *cross-base split* ([#960](https://github.com/rotnov/pycc/issues/960)): two independent sibling bases, one contributing an instance slot in its `__init__` and the other a same-named class attribute, are never compared with each other, because the check only walks a class's own `class_attrs` against its own MRO. That shape *is* modellable and is compiled with CPython's own answer — an instance `__dict__` entry shadows a non-data-descriptor class attribute, so the read resolves to the slot. `pycc_types`'s `resolve_attr_get` and `pycc_mir`'s instance `AttrGet` arm both order their walks properties → instance slots → class attributes to implement it, and `reject_class_attr_collisions` correctly stays as it is.
- **Names the interpreter reserves for itself are rejected in every spelling**, by `reject_reserved_class_attr_name` (`crates/pycc_hir/src/class/reserved_names.rs`). Three independent sets live there.
    - `__slots__`. A class's instance layout is already fixed at compile time from its `__init__` ([D-154](./decisions/D-154-class-instance-runtime-layout-stays-opaque.md)), so binding `__slots__` as an ordinary constant would silently discard the declaration. The name carries four separate accounts, one per reachable route — the plain/`@dataclass` attribute route (D-154, just stated), the `Enum` member list (CPython's `_EnumDict`, described in the enum row above), the `@property` getter (`type.__new__` iterating `__slots__` at class creation, [#980](https://github.com/rotnov/pycc/issues/980), described below), and every other class-body `def` spelling of the name ([#984](https://github.com/rotnov/pycc/issues/984), same mechanism as the getter but naming the carrier the binding form fixes — `function`, `staticmethod` or `classmethod`) — because each fails for a different reason and stating the wrong one is a false explanation rather than a stylistic slip.
    - `__init__`, `__new__` and `__init_subclass__` ([#975](https://github.com/rotnov/pycc/issues/975), [D-236](./decisions/D-236-reject-a-class-attribute-named-after-the-instantiation.md)). The generating rule is *the names Python's instantiation or class-creation protocol calls implicitly* — `C()` runs `type.__call__` → `__new__` → `__init__`, and `class B(C)` runs `__init_subclass__` at class creation. Binding one of them to a non-callable object makes CPython raise a `TypeError` at the use site (its text names the bound type, `'int' object is not callable` for an `int` binding, which is why pycc's own diagnostic omits the type instead of deriving it from an initializer it has not yet read), while pycc resolves each protocol without ever consulting a class attribute of that name: `ensure_init` decides whether to synthesize a constructor from the method table alone, the `__init_subclass__` hook is found by walking the MRO's function definitions, and `__new__` is not modelled at all. Every one of the three spellings above reached that silent divergence before #975, so the guard is deliberately **not** `ClassVar`-gated. It applies in *every* class body — plain, `@dataclass` and `Enum` — which is also what closes the two names the dataclass set below omits. The `@property def X` spelling is the fourth route into the same binding and is rejected too, by `reject_reserved_property_name` in the same module ([#978](https://github.com/rotnov/pycc/pull/978)'s review round): `walk_class_body` routes a getter to `MethodKind::PropertyGetter` rather than to the class-attribute path, so `@property def __new__(self) -> int` followed by `C()` compiled and ran here while CPython 3.13.9 raises `TypeError: 'property' object is not callable`. A plain `def __init__` is untouched — only the property spelling is rejected. `@property def __slots__` is rejected on that route too since [#980](https://github.com/rotnov/pycc/issues/980), but under a **third, distinct `__slots__` message** rather than the D-154 instance-layout one, which would not describe it: `type.__new__` iterates `__slots__` while the `class` statement itself executes, and a `property` object is not iterable, so CPython 3.13.9 raises `TypeError: 'property' object is not iterable` and never creates the class at all, while pycc lowered the getter as an ordinary property and ran the program. That message names `property` where the three protocol strings omit the bound type, because the decorator fixes the carrier type structurally instead of leaving it to an initializer the guard has not read. It takes no route parameter: a plain and a `@dataclass` body share that one arm and diverge identically, and an `Enum` body rejects method definitions outright before reaching it. [#984](https://github.com/rotnov/pycc/issues/984) then widened the same `__slots__` rejection to every *non*-`@property` `def` of that name under a **fourth** message: a bare `def`, `@override`, `@abstractmethod`, `@staticmethod` and `@classmethod`, in a plain and a `@dataclass` body, plus a `Protocol` body — which reaches its own call site in `lower_protocol_class`, because `lower_class` returns through that function before the method loop runs at all. The mechanism is the getter's, and the only per-spelling difference is the carrier CPython names (`function`, `staticmethod`, `classmethod`), which the binding form fixes structurally; D-236's #984 amendment generalizes the naming rule from "the decorator fixes the type" to "the binding form fixes the type" for exactly that reason. The *instantiation-protocol* half of the same guard deliberately stays `@property`-only: a plain `def __new__` binds the callable CPython's protocol expects, so it is out of D-236's rule and stays pending [#981](https://github.com/rotnov/pycc/issues/981). A `@__slots__.setter` with no preceding getter also stays on its existing "requires a preceding `@property` getter" diagnostic, since CPython raises `NameError: name '__slots__' is not defined` there rather than a `TypeError` about a non-iterable object. `@property def __qualname__` diverges as well (`TypeError: type __qualname__ must be a str, not property`) but is *value-typed* on the attribute route — `__qualname__: int = 1` diverges while `__qualname__: str = "D"` agrees — so it needs a generating rule this value-independent guard cannot host and is tracked as [#982](https://github.com/rotnov/pycc/issues/982). The rule is specifically about binding one of the names to a **non-callable** object — the literal text of all three messages — so a `def __new__` is outside it: `def __new__(self) -> int: return 7` followed by `c = C(); c.f()` still compiles and prints here while CPython 3.13.9 raises `AttributeError: 'int' object has no attribute 'f'`, because `type.__call__` binds `__new__`'s return value. Widening the rule to cover a callable binding (together with the implicitly-static `@staticmethod def __new__` spelling and the `@classmethod def __init_subclass__` twin) is [#981](https://github.com/rotnov/pycc/issues/981). `__init_subclass__` is the one conservative entry: inside an `Enum` body it can never diverge (an `Enum` with members cannot be subclassed), and inside a plain class body it diverges only once the class is actually subclassed, which the class-body walk cannot know; it is rejected in both for uniformity of the single rule. The five *other* names in the dataclass set below stay legal in a plain class body only because every rewrite consulting them is `is_dataclass`-gated today — ungating one is D-236's trigger to revisit.
    - The assignments CPython's `enum._EnumDict.__setitem__` keeps out of an enum's member list ([#979](https://github.com/rotnov/pycc/issues/979), [D-238](./decisions/D-238-reject-enum-body-assignments-cpython-keeps-out-of-the.md)), described in full in the `enum.Enum` row above. It is the only one of the three sets that is *route-gated* rather than merely route-worded — `ClassBodyRoute::Enum` decides whether the check runs at all, because a plain, `@dataclass` or `Protocol` body has no member list — and the only one keyed on the enclosing class's name. It runs *after* the two sets above, so every message pinned by #910, #975 and #984 is unchanged on the enum route even though all four of those names are dunder-shaped.
- A class attribute is **read-only**. Every write path is rejected, though not all under the same code: `obj.X = ...` and `self.X = ...` (inside or outside `__init__`) are `T0044` (``cannot assign to `X`: it is a class-level attribute ...``), while a class-name-qualified store `C.X = ...` is `T0021` (``name `C` is not defined``) — the store side has no class-name arm at all, so the class name is inferred as an ordinary value binding and fails there first. That is a coarser diagnostic than the read side's, and it holds for an inherited attribute (`Derived.X = ...`) exactly as for an own one; improving it is tracked with the rest of the write-side `T0044` family by [#965](https://github.com/rotnov/pycc/issues/965). This holds even for [#960](https://github.com/rotnov/pycc/issues/960)'s cross-base split, where the *read* now resolves to the sibling base's instance slot: the store-side check is coarse in the same way as the non-name-base read gate above, so the write stays rejected (conservatively) while the read is compiled correctly. Rebinding a folded constant has no representable meaning, and permitting `self.X = ...` inside `__init__` would let `collect_init_attrs` establish a colliding instance slot behind the collision check.
- A class name is only read *as a class name* when **no active value binding claims that name**. A parameter, local or global bound to `C` shadows the class exactly as CPython's own name resolution does, so `def f(B: D) -> int: return B.X` reads `B`'s instance slot and never the class `B`'s attribute. `pycc_types`'s class-name `AttrGet` arm guards on `binding_state(name).is_none() && !is_local(...)` — the same pair the class-name `Subscript` (`__class_getitem__`) path already used — and its class-name `MethodCall` arm — `ClassName.static_or_class_method(...)` — guards on the same pair, placed before the method-table walk so a shadowed name short-circuits. `pycc_mir` spells the guard as a lookup through the active scopes at each of its three matching interception sites: the enum-member interception, the class-attribute fold, and the class-name method call. A fourth `pycc_types` site asks the same question in the opposite direction: `rewrite_generic_calls_in_expr`'s expression walk in `monomorphize.rs`, which runs over every function body as soon as the module declares a PEP 695 generic function, *skips* a class-name base rather than guarding a fold. Recursing into one would infer a bare class name as a value and fail with `T0021` before either arm above ran; the outer `infer_expr_in` on the whole expression is what resolves the class-name read, so skipping the base loses nothing. Its `Subscript` (`__class_getitem__`) arm already skipped; #974's round widened the skip to the `AttrGet` and `MethodCall` arms behind one shared `is_class_name_base` predicate carrying the same binding/local pair, which is also what makes the shadowing rule hold inside a generic module. That `T0021` was **not** a #974 regression — an own class attribute and a `@staticmethod` call by class name failed identically before the MRO walk existed. The `Slice` arm needs no branch: `A[1:2]` on a class name is rejected with `T0021` by the checker itself, with or without a generic in the module, so no class-name base ever reaches it in a program that compiles. The shadowed name then falls through to the ordinary instance path in both crates instead of reaching the fold's internal-error panic. Four class-name shapes are covered: an inherited class attribute, a class attribute declared by the class itself, an enum member, and a `@staticmethod` or `@classmethod` called through the class name. Only the first was introduced by [#974](https://github.com/rotnov/pycc/issues/974)'s MRO walk (before it the shadowed inherited read was rejected with `T0044`); the other three silently read or called the class's own binding in every earlier version. One of the three shadow sources is narrower than it reads: a **module global** shadows a class name only where the shadowing is unambiguous, which is at module scope itself. `check_and_resolve_all_keyed`'s pass 2 walks top-level statements sequentially, so a top-level `C = [1, 2, 3]` shadows the class `C` from that statement on and a read above it still reaches the class -- source order decides, exactly as CPython's does. Pass 3 then checks every *function body* against the environment as it stands after all top-level code has run (D-041 late binding), so there the same `binding_state` answer is ordering-blind: for `def f() -> int: return A.X` with a top-level `A = D()`, CPython prints `2` when the rebinding executes after the call and `1` when it executes before, and one snapshot cannot say which. Since #974's round 5 that read is therefore **rejected with `C0001`** rather than resolved to either answer. The three `pycc_types` dispatch sites (`Subscript`, `AttrGet`, `MethodCall`) route the whole decision through one shared predicate, `expr::class_name_dispatch`, which returns "the class", "a shadowing value", or that rejection; `Environment::in_function_body` -- set only by `child_for_function` -- is what distinguishes the two positions. A **parameter or function-local** shadow is unaffected in either position: it is bound at the call, not by module top-level code, so `def f(B: D) -> int: return B.X` still reads the parameter. `pycc_mir`'s three interception sites need no matching branch, because the type checker rejects the ambiguous program before MIR runs. The **opposite source order** -- a top-level value binding followed by a `class` statement of the same name (`A = D()` then `class A: X: int = 2`) -- is not an ambiguity at all in CPython, which simply rebinds the name to the class; but pycc's HIR has no `HirItem::ClassDef` variant, so a class statement never appears in `HirModule::items` and pass 2's sequential walk cannot re-bind the name back. Every later read therefore resolved through the *stale value* binding -- `print(A.X)` printed `1` from `D`'s slot where CPython prints `2` -- a wrong answer rather than a diagnostic. #974's round 6 rejects that collision with `C0001` at HIR lowering instead, in `crates/pycc_hir/src/module.rs`'s `Stmt::ClassDef` arm, alongside the four collision checks (class, function, type alias, import) that arm already runs; `killed_names` supplies the "which names do the statements lowered so far bind" answer. Two consequences are deliberate and narrower than Python: the rejection covers a program CPython accepts, and it is unconditional -- it fires whether or not anything ever reads the name afterward. The check inspects only *earlier* items, so the class-then-value order above is untouched.
- A **class-name-qualified** read (`C.X`) resolves through `C`'s MRO, most-derived-first, and folds to the class attribute declared by the first MRO class that declares `X` at all — provided that class declares it *as a class attribute*. If some other class-level namespace of that class claims the name first (a method, `@staticmethod`, `@classmethod`, `@property`, an enum member, or a `Protocol` body's own `def`), the read stays `T0044`: CPython binds that declaration instead, so folding the base's constant would be a wrong value rather than a missing feature ([#974](https://github.com/rotnov/pycc/issues/974)). `pycc_types`'s `lookup_class_attr_by_class_name` and `pycc_mir`'s class-name fold run the identical walk over one shared `pycc_hir::declares_name_outside_class_attrs` predicate, so the checker's answer and the folded constant cannot drift — the #960 failure mode. This walk deliberately **does not** apply the instance path's [#960](https://github.com/rotnov/pycc/issues/960) precedence, and it is not `lookup_class_attr_through_mro`, which still serves the instance path and its other callers unchanged. A class object has no instance `__dict__`, so an instance slot never shadows here, and CPython agrees: for `class A` establishing an instance slot `x = 1` in its `__init__`, `class B` declaring `x: int = 2`, and `class C(A, B)`, `c.x` is `1` while `C.x` is `2` — pycc compiles both, in one program. A protocol member splits on its spelling. A bare `x: int` requirement does not shadow, for the same reason as an instance slot: it is an interface requirement, not a binding on the class object (`class P(Protocol): x: int` / `class A: x: int = 2` / `class C(P, A)` puts `P` first in the MRO and still reads `2`). A `def x(self) -> int: ...` in that same body *does* shadow: a `Protocol` class executes its body like any other class, so the name really is bound in `P.__dict__`, and CPython 3.13.9 resolves `C.x` to that function object rather than continuing to `A` — folding `2` there would be a mis-compile both crates agree on, which is precisely what the shared predicate cannot catch on its own. One shape is not covered by this bullet: a PEP 695 generic class (`class Box[T]: LIMIT: int = 8`) accepts `Box.LIMIT` under `pycc check` and then fails `pycc build` with `T0021`, a check/build divergence that predates #974 and is tracked by [#989](https://github.com/rotnov/pycc/issues/989).
- A read must go through a plain name: `C.X`, `obj.X`, or `self.X` -- or through `super()` ([#915](https://github.com/rotnov/pycc/issues/915)), which is exempt because a `super()` expression is side-effect-free. Reading off any other base expression (`make().X`) is rejected with `T0044`, because folding the read would discard the base expression and its side effects. That gate is deliberately coarse: it fires whenever a class attribute of that name exists anywhere in the MRO, without asking whether a sibling base's instance slot shadows it under [#960](https://github.com/rotnov/pycc/issues/960)'s precedence — a conservative rejection of a program CPython accepts, never a mis-compile.
- A class attribute **satisfies a `Protocol` attribute member** ([#914](https://github.com/rotnov/pycc/issues/914)), exactly as an instance slot or a `@property` does, matching mypy and pyright; the member's type is checked identically in all three cases, and a mismatch is the same `T0046`. Two seams enforce that jointly: `pycc_types`'s `check_protocol_conformance` falls back to the full-MRO `class_attrs` walk when the instance/property walk misses, and `pycc_mir`'s `eval_isinstance_protocol` matches `class_attrs` too, so a `@runtime_checkable` `isinstance` cannot disagree with the checker (#380 W2). The instance/property walk deliberately runs *first*: two independent sibling bases can contribute an instance slot and a same-named class attribute to one derived class without the collision check above ever comparing them, and CPython lets the instance `__dict__` entry win there (the same shape's value divergence for an ordinary attribute read was [#960](https://github.com/rotnov/pycc/issues/960), now fixed the same way — `pycc_mir` looks for the slot before folding). What is **not** modelled is a protocol member that is written through: pycc tracks no write through a protocol-typed receiver at all, for an instance attribute just as much as for a class attribute, so admitting class attributes widens nothing — the tracked follow-up is [#958](https://github.com/rotnov/pycc/issues/958). Every direct write path to a class attribute stays `T0044` regardless.

In a `@dataclass` body a bare assignment keeps its pre-[#910](https://github.com/rotnov/pycc/issues/910) `C0001`: there it is Python's class-level *default* for a field declared elsewhere, not a constant, and field defaults are their own deferred feature.

#### `ClassVar` in a `@dataclass` body ([#913](https://github.com/rotnov/pycc/issues/913), [D-235](./decisions/D-235-reject-a-dataclass-field-that-shares-its-name-with-a.md))

An *annotated* `ClassVar` declaration in a `@dataclass` body is an ordinary class-level attribute, taking exactly the route described above — PEP 557 says a `ClassVar`-annotated name is **not** a dataclass field. It therefore never enters the field list, and because `lower_class` fills the D-154 instance-slot layout (`HirClassDef::attrs`) from that same merged field list, it is absent from four things at once: the synthesized `__init__`, `__eq__` and `__repr__`, and the instance layout. Every restriction of the class-attribute model applies unchanged there — scalar type, literal right-hand side, no `__slots__`, read-only, `Final` nesting rules — so a value-less `ClassVar` (`LIMIT: ClassVar[int]`) stays `C0001` inside a dataclass body exactly as it does outside one, even though CPython accepts it. A derived dataclass overriding a base dataclass's `ClassVar` is an ordinary override, not a collision.

Four shapes are rejected, all of them because CPython's answer is a dataclass field *default* or an order-dependent field removal, and this version models neither:

- **A field and a `ClassVar` of the same name**, whether they meet in one body, across an inheritance edge, or across two sibling bases. CPython's answers are mutually inconsistent: in one body, declaring the field first drops the field while declaring the `ClassVar` first turns it into a field whose default is the class attribute's value; across an inheritance edge (`class B(A)` where `A` declares the `ClassVar` and `B` the field) the base's value becomes the field's default, so `B.__init__` takes an *optional* parameter where a synthesized one would take a required one. Across sibling bases the answer depends on base order alone: with `class D(A, B)` where `A` contributes `LIMIT: ClassVar[int]` and `B` a `LIMIT: int` field, CPython processes fields in reverse-MRO order, so `A`'s `ClassVar` removes `B`'s field entirely — while `class D(B, A)` keeps it. `D(B, A)` is consequently a **deliberate conservative rejection of a program CPython runs and pycc could compile**: the two orders are indistinguishable to a reader, and the other one is a mis-compile. The check runs on the *merged* field list, after the field merge and before the synthesis, which is the only place all three shapes meet. This is a different shape from [#960](https://github.com/rotnov/pycc/issues/960)'s "cross-base split" described a few bullets above, which is **accepted**: that one is a non-dataclass *instance slot* versus a class attribute, resolved by read precedence; this one is a dataclass *field* versus a `ClassVar`, and it is rejected.
- **A derived `ClassVar` shadowing a field inherited from a base dataclass** (`@dataclass class A: v: int` / `@dataclass class B(A): v: ClassVar[int] = 5`). CPython drops `v` from `B.__init__`. The existing collision check already rejects it, because a base dataclass's `attrs` *is* its merged field list; the rejection is kept deliberately, and only became reachable with #913.
- **A `ClassVar` named after a dunder the dataclass relies on implicitly** — `__init__`, `__eq__`, `__repr__`, `__ne__`, `__str__`, `__format__` (`DATACLASS_IMPLICIT_DUNDERS` in `crates/pycc_hir/src/class/body.rs`). CPython's `dataclasses` uses `_set_new_attribute`, which leaves an existing class `__dict__` entry alone, so the class attribute wins and the method is not synthesized at all. pycc synthesizes the first three unconditionally, and it also rewrites `!=` through the synthesized `__eq__` (never consulting `__ne__`) and both `print(instance)` and f-string interpolation through the synthesized `__repr__` (never consulting `__str__` or `__format__`) — so all six spellings compiled to a silently different answer where CPython 3.13.9 raises `TypeError: 'int' object is not callable`. The set is generated by a rule, not enumerated: *the three synthesized methods, plus every dunder CPython consults for an operation pycc rewrites through one of them*; adding a new implicit dataclass rewrite is a trigger to revisit it. A dunder pycc does not rewrite stays legal — `__hash__: ClassVar[int] = 8` agrees with CPython (`dataclasses` leaves an explicitly bound `__hash__` alone), and `__lt__` is already `T0021` before MIR. `__slots__` keeps its own earlier, separate rejection. Two names the rule *cannot* generate — `__new__` and `__init_subclass__`, which Python calls implicitly rather than through anything pycc rewrites — had the same divergence and are covered instead by the universal reserved-name guard described above ([#975](https://github.com/rotnov/pycc/issues/975), [D-236](./decisions/D-236-reject-a-class-attribute-named-after-the-instantiation.md)), which supersedes this set's *generating rule* while leaving the six names and their messages exactly as [D-235](./decisions/D-235-reject-a-dataclass-field-that-shares-its-name-with-a.md) accepted them. The two sets are kept disjoint deliberately: the dataclass check runs first, so `__init__` in a `@dataclass` body still reports the message above.
- Everything the class-attribute model rejects anywhere, unchanged: a value-less `ClassVar`/`Final` declaration, a non-scalar type, a non-literal right-hand side, `__slots__`, and the two PEP 591-invalid `Final`/`ClassVar` nestings.

A program carrying two defects reports the earlier check: everything raised during the class-body walk first, then the class-attribute collision check, then the `T0052` field-type conflict raised inside the merge loop, then the field/`ClassVar` name check, then [#969](https://github.com/rotnov/pycc/issues/969)'s multiple-inheritance slot-layout gate.

The asymmetry that used to survive here — a class-name-qualified read of an **inherited** class attribute (`Derived.LIMIT` where only `Base` declares it) rejected with `T0044` while `derived_instance.LIMIT` succeeded — was fixed by [#974](https://github.com/rotnov/pycc/issues/974). It was never dataclass-specific; a `@dataclass` was simply the shape that made it visible. Both forms now resolve, under the two different precedence rules stated in the read bullet above.

(`super().X` *does* now reach a base class's class attribute, [#915](https://github.com/rotnov/pycc/issues/915); `Final[...]` *is* now accepted on a class-body attribute, [#916](https://github.com/rotnov/pycc/issues/916) — see the `Final[X]` section above; and a class attribute *does* now satisfy a `Protocol` attribute member, [#914](https://github.com/rotnov/pycc/issues/914) — see the bullet above.)

## Python 3.15 typing preview (post-v1.0)

The preview does not expand the v1 Python 3.14 contract. The v1.x language
upgrade in ROADMAP.md adds four type-system obligations: singleton-value typing
and `is` narrowing for `sentinel()` (PEP 661), typed `TypedDict` extra items
(PEP 728), `TypeForm` (PEP 747), and disjoint-base reasoning (PEP 800). Each is
tracked by its own `py315/` conformance row in PYTHON_STANDARDS.md.

## Class model (compiled subset)

Supported: single inheritance with C3 MRO resolved at compile time, and multiple inheritance restricted to base sets whose instance-attribute layouts do not conflict (see "Multiple inheritance and the flat slot layout" below), `@property`, `classmethod`/`staticmethod`, `__init_subclass__` (PEP 487 — recognized as a regular method; full invocation (CPython calling it automatically at subclass creation) is deferred, [D-213](./decisions/D-213-defer-pep-487-full-invocation-reject-the.md)/[#585](https://github.com/rotnov/pycc/issues/585). Within that deferral, a single, unconditional check runs at every class's own creation, regardless of whether that class also defines its own `__init_subclass__` override ([D-214](./decisions/D-214-unify-the-init-subclass-guard-into-a.md)/[#854](https://github.com/rotnov/pycc/issues/854)): the nearest ancestor in the class's own MRO that defines `__init_subclass__` (found by a decorator-agnostic scan of its raw AST, so a `@classmethod`-decorated hook is covered identically to a plain one) must be statically evaluable — only `pass` or a docstring body is accepted, since pycc has no mechanism to run side-effecting statements at class-creation time. This matches CPython's actual invocation target: `super(new_cls, new_cls).__init_subclass__(...)` in `type_new_init_subclass` starts its MRO lookup immediately after `new_cls`, so `new_cls`'s own `__init_subclass__` definition is never the body CPython invokes at `new_cls`'s own creation — it only matters later, when something subclasses `new_cls` in turn. The violation is rejected at the class's own creation site (not the ancestor's definition site), while a base class that is never subclassed stays legal regardless of its `__init_subclass__` body's shape. A direct consequence: a class sitting directly beneath a side-effecting, introspectable ancestor hook is illegal at its own creation no matter what it overrides the hook with, since its own creation is checked against that ancestor, not its own override), `__set_name__` (PEP 487 — recognized as a valid method name; not triggerable yet because a class-level attribute is restricted to a scalar slot type (`int`/`float`/`bool`/`str`), so no *descriptor-valued* class attribute — the sole trigger CPython has for the hook — can exist. A descriptor-valued class attribute is rejected with `C0001` at HIR-lowering time; see "Class-level attributes" below. Before [#911](https://github.com/rotnov/pycc/issues/911) this note attributed the untriggerability to class-level attribute assignments being unsupported outright, which is no longer the reason. The deferral itself is unchanged: [D-213](./decisions/D-213-defer-pep-487-full-invocation-reject-the.md)/[#585](https://github.com/rotnov/pycc/issues/585)), `__class_getitem__` (PEP 560 — in value position, `ClassName[key]` dispatches to the class's own `__class_getitem__` through the MRO, in either the `@staticmethod` or the `@classmethod` spelling, and the subscript's type is that hook's return type; a class that defines no such hook anywhere in its MRO is rejected with `T0044`, matching CPython's `TypeError: type 'C' is not subscriptable`. A name bound as a value shadows the class, so `C[0]` then indexes that value instead. In annotations, `ClassName[type_arg]` is accepted whenever the base resolves to a type pycc can name nominally, with the type argument **erased without being lowered** ([#1130](https://github.com/rotnov/pycc/issues/1130)); the subscriptability gate [#611](https://github.com/rotnov/pycc/issues/611) applied here is gone, because PEP 649/749 evaluates annotations lazily, so the `TypeError` value position raises is unreachable from annotation position in a pycc-compiled program (pycc builds no runtime `__annotations__` object, so nothing can force it). Once accepted, the annotation's resolved type depends on *why* the class is subscriptable ([#693](https://github.com/rotnov/pycc/issues/693)): when an explicit `__class_getitem__` hook exists somewhere in the MRO, the annotation resolves to that hook's declared return type — mirroring value position's own use of the declared return type, not a runtime evaluation of the hook's body — found by walking the MRO most-derived-first, exactly as the value-position dispatch does; when no hook exists — whether the class is a PEP 695 generic or a plain non-generic one — the annotation still resolves to `Ty::Instance(ClassName)`, ignoring the type argument, except where the class shadows one of the six names the bare-name arm answers *before* it consults the class table (`int`, `float`, `bool`, `str`, `Any`, `memoryview`), where that reserved meaning wins and the subscripted form follows it ([#1130](https://github.com/rotnov/pycc/issues/1130)), since actual generic instantiation is a separate, already-existing mechanism (`GenericClassInstantiate`), not this annotation path. One narrow case still falls back to `Ty::Instance(ClassName)` even when a hook exists: a `ClassName[type_arg]` annotation used *inside* `ClassName`'s own body, referring to the very class being lowered — the hook's return type is not yet resolvable at that point in a single left-to-right lowering pass. A base that is not a known class resolves as the bare name would: the four builtin containers lower per D-228; a `type A = C` alias to a class is transparent, so `A[int]` behaves exactly as `C[int]`; a PEP 695 type parameter, a builtin scalar, `Self`, or an alias to a non-class type is rejected with `T0044` ([#931](https://github.com/rotnov/pycc/issues/931)) rather than having its type argument silently discarded — with the buffer carrier carved out of that last category, since `type Arr = memoryview` then `Arr[float]` resolves to the carrier and is accepted (#1130); and an undefined name keeps its `C0001`), dataclasses (557) and `dataclass_transform` (681), `@override` (698) enforced, dunder protocol methods (`__len__`, `__iter__`, `__enter__`…) → static dispatch.

Subclassing a builtin type is not supported yet: a base naming one is reported as `C0001` "class `X` inherits from builtin type `T`" rather than as an unknown class; which names and which rebinding exceptions apply is specified in the [DIAGNOSTICS.md](./DIAGNOSTICS.md) paragraph that begins "A class whose base names one of the eleven builtin types" (Part 1 of [#1283](https://github.com/rotnov/pycc/issues/1283)); hosting a `frozenset` subclass is tracked by [#1319](https://github.com/rotnov/pycc/issues/1319).

### Multiple inheritance and the flat slot layout

Under [D-154](./decisions/D-154-class-instance-runtime-layout-stays-opaque.md) an instance is a flat array of attribute slots, and each method is lowered exactly **once**, against its own class's layout. A derived class's layout is assigned most-base-first over its MRO, so a base whose own layout is not a *prefix* of the derived layout has its already-lowered methods addressing the wrong slots on a derived instance.

Since [#969](https://github.com/rotnov/pycc/issues/969), `pycc_hir` rejects exactly that shape with `C0001` ([D-234](./decisions/D-234-reject-multiple-inheritance-whose-base-layouts-are.md)). The predicate is a **name-sequence prefix test**: for a class with more than one base, every class in its MRO must have a flat layout whose attribute-name sequence is a prefix of the derived class's. Concretely:

- **Accepted:** single inheritance of any depth; a methods-only mixin or a class-attribute-only base as a second base (neither establishes an instance slot); two bases declaring the *same* attribute names (they share one slot); a builtin exception base (its synthetic class def carries no attributes); a diamond whose intermediates add nothing, and a one-sided diamond where only one branch adds an attribute.
- **Rejected:** two bases that each declare their own, differently named instance attributes — including a diamond `Base`/`A(Base)`/`B(Base)`/`C(A, B)` in which **both** `A` and `B` add an attribute of their own.

The rejection is a deliberate narrowing rather than a layout redesign: for two classes in one MRO whose layouts agree up to slot *k* and then name different attributes at slot *k*, no single flat ordering puts both names at index *k*, so no reordering of `mro_attrs` can fix the shape. It is also not refinable to "reject only when the mis-addressed member is reachable", because `super()` can re-enter a member the derived class shadows. Lifting the restriction requires per-derived-class re-lowering of inherited methods or a runtime name-to-slot indirection; D-234 records that as the tracked future path. End-to-end tests: `tests/issue_969_mi_slot_layout.rs`.

Rejected (negative-tested, see DIAGNOSTICS.md): runtime class mutation, dynamic `type()` creation, metaclasses with runtime side effects beyond the statically evaluable subset (`E0105`), custom `__getattr__` catch-alls on non-interop types (`E0102`).

**Current state (through PR-16, D-154, #387, #377, #432, #433, #436, and #378):** this is the v1.0 target model above, partially landed — PR-15 ships real codegen for a **single class** (instance attributes established by a first assignment inside `__init__` (each `self.<attr> = <expr>` establishes one `i64`-slot attribute, in source order, from a bare reference to one of `__init__`'s own always-annotated parameters or a scalar int/float/bool/str literal; since [#1262](https://github.com/rotnov/pycc/issues/1262) the parameter may also be a `list[int]` or `dict[str, int]`, whose slot word is the container's pointer — leak-only per D-107/D-124, so a read yields the same object and a mutation through an alias (`ys = b.xs; ys.append(4)`) is visible through the attribute, as in CPython (oracle fixture `tests/fixtures/instance_container_slots.py`, end-to-end tests `tests/issue_1262_container_slots.rs`). Since [#1264](https://github.com/rotnov/pycc/issues/1264) (#1218's Part 3) an annotated empty initialiser `self.xs: list[int] = []` or `self.d: dict[str, int] = {}` also establishes such a slot, typed by its written annotation, and each `__init__` call allocates a fresh container. The same annotated statement is accepted in any function body as a store to an existing slot, with a slot-type mismatch reported as `T0021` and an undeclared attribute as `T0044`. `pycc_hir` builds the typed empty container from the annotation (D-245's 2026-09-24 amendment). Every other annotated attribute target is refused, and the first failing check wins, in this order: module level, a `super()` base, `Final`, the annotation, a missing value, the value's shape, and the base expression. Module level, `Final`, a missing value, and a well-formed annotation whose value is not its matching empty literal are `C0001` naming the admitted form. That last case covers any annotation other than `list[...]`/`dict[...]` (a scalar, `set[int]`, `tuple[...]` or an instance type) and any non-empty or mismatched value. A `super()` base keeps #448's `C0001`. An annotation that `annotation_to_ty` itself rejects keeps exactly the diagnostic a local's annotation would get, such as `T0034` or the generic unknown-name `C0001`. The one exception is a bare `list`/`dict`, which gets D-228's parameterized-form advice. A base expression that cannot be lowered keeps its own diagnostic. The general form is [#891](https://github.com/rotnov/pycc/issues/891) (oracle fixture `tests/fixtures/instance_annotated_container_slots.py`, end-to-end tests `tests/issue_1264_annotated_init_containers.rs`). Since [#1265](https://github.com/rotnov/pycc/issues/1265) (#1218's Part 4) an *unannotated* `self.xs = []` at the top level of `__init__` also establishes a `list` slot. `pycc_hir` lowers it to a provisional slot, and the empty-container pass types it before any checking. It takes the element type from the first concrete, same-shape slot a base class declares for the attribute, or else from the first `self.xs.append(v)` in statement position in the class's own methods (the method table and property accessors, in source order, never a static or class method). A producer whose value does not infer ends the search. The pass also types every `self.<attr> = []`/`{}` reset in those methods from the slot. A slot no source types is `T0003` naming the attribute and the class, ahead of every other check. An unannotated `self.d = {}` establishing the attribute in `__init__` stays `C0001` until its `self.d[k] = v` producer exists (#891), unless a class-body declaration types it (below) (D-245's 2026-09-24 amendment for #1265; oracle fixture `tests/fixtures/instance_unannotated_list_slots.py`, end-to-end tests `tests/issue_1265_unannotated_init_list.rs`). Since [#1266](https://github.com/rotnov/pycc/issues/1266) (#1218's Part 5) a value-less class-body annotation that is neither `ClassVar`- nor `Final`-wrapped (`n: int`, `items: list[int]`, `v: T`, also inside `Annotated[X, ...]`) is an **instance attribute declaration**, as in CPython: it creates no class attribute and no slot by itself, and fixes the type of the slot the class's own `__init__` establishes at top level (`pycc_hir::class::declared_attrs`). The declared type replaces the one the establishing value would infer, so `self.d = {}`/`self.xs = []` take a declared `dict[str, int]`/`list[int]`, and the value is then checked against the slot like any later assignment: an `int` under a declared `float` is `T0021`, and an empty literal of the wrong shape (`{}` under `list[int]` or `int`, `[]` under `dict[str, int]` or a type parameter) is `T0003`. The shape gate on a non-empty establishing value is unchanged, an annotated establishing assignment must agree with the declaration, and a type parameter and a concrete type never meet in one slot. Each refusal is its own `C0001`: a declared type other than `int`/`float`/`bool`/`str`, a type parameter, `list[int]` or `dict[str, int]` (a bare `list`/`dict` gets the parameterized-form advice, and an element type keeps its `T0034`-family code); a duplicate declaration; a declaration named like a method, property, static or class method of the same body; and a declaration the own `__init__` never assigns at top level (none, only a nested block, only another method, or only an ancestor's `__init__`), which also suggests the class-constant spelling for a scalar. A `@dataclass` body keeps the spelling's field meaning and a `Protocol` body its attribute meaning (D-245's 2026-09-24 amendment for #1266; oracle fixture `tests/fixtures/instance_declared_attrs.py`, end-to-end tests `tests/issue_1266_class_body_declarations.rs`). `set`/`tuple`- and instance-typed attributes are still rejected pre-codegen, and `for x in self.xs` is `I0404` (iterate a local alias instead). Since [#1263](https://github.com/rotnov/pycc/issues/1263) (#1218's Part 2) `.append(v)`, `.pop()` and `.get(k, default)` also accept an instance-attribute receiver (`self.xs.append(v)`, `b.d.get(k, 0)`, or a `@property` read such as `h.items.append(v)`), mutating or reading the container the slot holds; the receiver is evaluated before the call's arguments, as in CPython, and any other receiver shape (`f().append(1)`, `math.pi.append(1)`) is `C0001` "`.append()` is only supported on a name or an instance attribute so far", an attribute of a CPython object (`sys.argv.append("x")`) is `I0404`, while `.add()` stays bare-name-only because no `set` slot exists (oracle fixture `tests/fixtures/attr_container_methods.py`, end-to-end tests `tests/issue_1263_attr_container_methods.rs`); the attribute-slot pre-scan only considers `__init__`'s own top-level body statements, with no recursion into a nested `if`/`while`/`for`, so a `self.x = 1` (or an annotated `self.xs: list[int] = []`) written only inside such a nested block establishes no slot at all — a later read of `self.x` then fails with `T0044` rather than an immediate, self-explanatory error at the assignment site itself), plain instance methods taking a receiver spelled with any identifier (#1181 -- `def area(this)` is accepted and lowered under the canonical parameter name `self`, so long as the method neither mentions the identifier `self` nor rebinds its receiver; `crates/pycc_hir/src/class/receiver.rs` owns that rule) (mangled `<ClassName>.<method_name>`, resolved to a compile-time-known function pointer — static dispatch, no vtable), attribute read/write (`base.attr` / `base.attr = value`), method calls (`base.method(...)`), and instantiation (`ClassName(...)`, calling the mangled `__init__`). The runtime instance object is an opaque heap object accessed only through FFI accessors, not a `#[repr(C)]` direct-layout struct (D-154's own ADR). **[#377] `@property`** getter and `@<name>.setter` are now implemented: a `@property`-decorated method is lowered into a `PropertyDef` entry on the class (not the methods table), and `base.attr` / `base.attr = value` are transparently rewritten to calls to the getter (`<ClassName>.<attr>`) and setter (`<ClassName>.<attr>.setter`) respectively, at the HIR/MIR level — no new MIR or codegen variant is needed, the existing method-call infrastructure is reused. A read-only property (getter only, no setter) rejects assignment with `T0044`; a setter type mismatch produces `T0021`. `classmethod`/`staticmethod` are now implemented (#436, see below); dunder protocol dispatch remains a v1.0-target follow-up (`docs/ROADMAP.md`), not yet implemented. **[#387] PEP 695 generic classes** (`class C[T]:` with a single type parameter) are now partially implemented: scalar-only instantiation (`C[int](args)`) is monomorphized at compile time (each `(class, type_arg)` pair produces a specialized class with substituted attribute types and method signatures, reusing PR-13's `substitute_ty`/`mangle_generic_instantiation` infrastructure), `Self` (PEP 673) resolves to the class's own instance type at HIR-lowering time, and self-referential class-name annotations in own methods work (PEP 649/749 deferred-evaluation semantics for the class-model scope) -- both resolve to the protocol type rather than an instance type when the enclosing class is a protocol ([#948](https://github.com/rotnov/pycc/issues/948)). Two or more type parameters, container-position type parameters, and non-scalar instantiation remain unimplemented — a bare attribute/method access on a non-instance value is `T0043`, an unknown attribute/method name is `T0044`, and any other class-body statement shape (a `def` other than `__init__`/a plain method, or a top-level statement that is neither a class-attribute assignment nor a `self.<attr> = ...`) is `C0001` (a class docstring -- a bare string-literal expression statement, accepted anywhere in the body -- is exempted from this rejection, [#744](https://github.com/rotnov/pycc/issues/744)). A method may be named `append`, `pop`, `get`, or `add` ([#1188](https://github.com/rotnov/pycc/issues/1188)). `pycc_hir` recognizes `x.append(v)`, `x.pop()`, `d.get(k, default)`, and `s.add(v)` as container calls before any type exists, so in a module that can see a user class (or protocol) defining one of those names -- the module itself or its transitive imports, package `__init__`s included -- it lowers both readings into one `HirExpr::ReceiverDispatchedCall`. `pycc_types` and `pycc_mir` then pick a reading with one shared rule, `pycc_hir::receiver_takes_method_path`: on a class-name receiver of a static or class method, or a receiver whose static type is a user-class instance or a protocol, the method wins; on every other receiver (`list`, `dict`, `set`, a foreign `object`, a scalar, or a receiver whose type cannot be inferred) the container form applies, with exactly its diagnostics. A module that can see no such method lowers these calls exactly as before. End-to-end tests: `tests/issue_1188_container_method_names.rs`. A class name colliding with an already-defined top-level function, type alias, or import name is likewise rejected with `C0001`, in either definition order. Class-body execution order and redefinition/rebind semantics: a non-`__init__` method redefinition within one class body rebinds (the second `def` replaces the method table entry, and the latest definition is dispatched to at runtime via PR #358's function-pointer slot -- both definitions share the same mangled `<ClassName>.<method>` name); `__init__` redefinition stays `C0001` (the attribute-slot pre-scan cannot reconcile two different `__init__` bodies); an incompatible method redefinition (different signature) is rejected with `T0021` by `check_incompatible_redefinitions`, which covers class methods through the flat `hir.items` list (#386). **[#432] Inheritance, C3 MRO, and `@override`** are now implemented: `class Derived(Base):` parses base class names, computes the C3 linearization at compile time, and stores it in `HirClassDef.mro`; method, attribute, and property resolution walks the MRO (most-derived-first), so a method or attribute declared in a base class is found when accessed on a derived instance, and a subclass method shadows a base class method of the same name. A derived class without its own `__init__` inherits the base class's constructor (resolved via the MRO at instantiation time). **[#912] A class with no `__init__` anywhere in its MRO** is no longer `C0001`: HIR lowering synthesizes an implicit zero-argument constructor for it ([D-225](./decisions/D-225-synthesize-an-implicit-zero-argument-constructor.md)), so `class Config:` with only class attributes, or `class A: pass`, is instantiable as `Config()`/`A()` exactly as CPython's inherited `object.__init__` makes it. The synthesized constructor takes only `self`, has an empty body, and establishes no attribute slots; calling it with any argument is `T0021`. Synthesis is per-class and only when nothing is inherited, so `class A: pass` followed by `class B(A): pass` gives `B` the inherited `A.__init__`, not a second synthesized one. **[#966] That synthesized constructor ranks last in constructor resolution** ([D-232](./decisions/D-232-rank-an-implicit-object-style-constructor-last-in.md)), where CPython ranks the `object.__init__` it stands for. Because it lands in its class's *own* method table, it would otherwise out-rank a real `__init__` on a later base: for `class C(A, B)` with `A` init-less and `B` declaring one, `C()` ran `A`'s empty stub, left `B`'s slots uninitialized, and aborted in `pycc_rt` on the first read, while the checker rejected `C(5)` with `T0021`. All five constructor-resolving seams -- instantiation and `super().__init__()` in both `pycc_types` and `pycc_mir`, plus the D-189 raisability walk -- now skip such a class, falling back to it only when every candidate in the MRO is implicit. The rank keys on *provenance* recorded at synthesis time, never on the constructor's shape: an explicitly written `def __init__(self) -> None: pass` lowers to the identical empty body, is a real constructor, and keeps ranking first, matching CPython's `AttributeError` for that program. A `@dataclass`'s generated constructor is likewise real and unaffected. Two limitations remain. The rule is scoped to constructor resolution, so the *generic* method-call surface is deliberately unranked: `c.__init__()` on an already-constructed instance still resolves through the name-agnostic MRO walk to the implicit stub and silently no-ops, and `c.__init__(5)` is still `T0021` on a class whose `C(5)` now works. And a class whose bases' instance layouts diverge used to hit a pre-existing MRO **slot-aliasing** defect in `mro_attrs`, independent of #966: the derived class's flat layout is assigned most-base-first while each base's own methods address slots against that base's own layout, so for `class B: self.z` / `class D: self.w` / `class C(B, D)`, reading `c.z` aborted and reading `c.w` returned a silently wrong value where CPython raises `AttributeError`. Since [#969](https://github.com/rotnov/pycc/issues/969) that shape is **rejected with `C0001`** at HIR-lowering time instead ([D-234](./decisions/D-234-reject-multiple-inheritance-whose-base-layouts-are.md)); see "Multiple inheritance and the flat slot layout" below. Both reads are pinned as rejections in `tests/issue_966_inherited_init_rank.rs`. An enum class early-returns before this point and keeps its constructor-less shape by design; calling it (`Color()`, `Color(1)`) is rejected with `C0001` at the call expression by `pycc_hir`'s per-item scan (`class::enum_call`, [#944](https://github.com/rotnov/pycc/issues/944)) whenever the scan can attribute the call, with `pycc_types`'s span-less `resolve_instantiation` guard as the backstop (reporting at `1:1`) for the scan's documented residuals before its `__init__` MRO walk, so the constructor-less shape never reaches `pycc_mir` ([#921](https://github.com/rotnov/pycc/issues/921)). The instance's flat attribute-slot layout is computed from the MRO — each unique attribute across the MRO gets one slot, with the most-derived class's declaration winning on name conflicts when every declaration of that attribute name in the MRO agrees on its type. **[#676] A differing-type conflict is rejected instead** (D-210): if two distinct classes in one class's own MRO each declare an attribute of the same name with a different type — including a diamond conflict between two sibling base classes that neither is the other's ancestor — `T0052` is emitted at class-definition time, since pycc compiles each method body exactly once (static dispatch, no per-subclass monomorphization) and has no per-instance runtime type tag to safely coerce at the shared assignment site (e.g. `class Base: def __init__(self) -> None: self.v = 0` and `class Derived(Base): def __init__(self) -> None: self.v = False` conflict on `v`'s type and are rejected; a same-class `bool`-into-`int` assignment stays accepted, D-187). `@override` (PEP 698) is recognized as a method decorator and validated: if the decorated method does not override a same-named method or property in any base class (walking the MRO, excluding the current class), `T0031` is emitted. Circular inheritance, duplicate bases, inheriting from a generic class, and an inconsistent C3 linearization are all rejected with `C0001`. `isinstance`/`issubclass`, `__init_subclass__`/`__set_name__`, and `__class_getitem__` are now implemented (#435, see below). **[#433] Zero-argument `super()`** and base class initialization are now implemented using static dispatch (D-160): `super().__init__(args)` and `super().method(args)` resolve the target starting from the next class after the current defining class in the MRO, with `self` as the implicit first argument — no vtable, no runtime polymorphism. A bare `super()` or `super()` outside a method body is rejected with `C0001`. `super().attr` resolves only the *class-level* members a CPython `super` object actually proxies along the MRO -- today that means a base class `@property`, whose getter is called directly, and a base class *class attribute* ([#911](https://github.com/rotnov/pycc/issues/911)), which folds to its literal ([#915](https://github.com/rotnov/pycc/issues/915)). Because a `super` object never sees the instance `__dict__`, a class attribute contributed by one MRO branch is found even when an unrelated sibling base establishes an instance attribute of the same name. A zero-argument `super()` inside a `@classmethod` or `@staticmethod` is rejected with `C0001`: those method kinds bind no `self`, and pycc's static `super()` lowering has no receiver to pass. An *instance* attribute (one established by `self.<attr> = ...` inside `__init__`, living in the instance's own slot) is not proxied by a `super` object and is rejected with `T0047` (#587), matching CPython's `AttributeError: 'super' object has no attribute '<attr>'`; the equivalent read uses the method's own receiver. A name declared nowhere in the MRO is `T0044`. **[#436] `@classmethod` and `@staticmethod`** are now implemented: `@classmethod`-decorated methods receive an implicit `cls` parameter typed as the defining class, `@staticmethod`-decorated methods have no implicit parameter, both use distinct mangled names (`<ClassName>.<method>.classmethod`/`.static`) and are dispatched statically through both class and instance syntax. Inherited static and class methods are resolved via the MRO. Same-name collisions between static/class methods and regular methods, properties, or the opposite decorated kind are rejected with `C0001`. **[#378] Dataclasses (PEP 557) and `dataclass_transform` (PEP 681)** are now implemented as a narrow, statically dispatched feature: `@dataclass` (bare) and `@dataclass_transform()` (bare, keyword arguments accepted but ignored) trigger auto-generation of `__init__` (one required parameter per annotated field, in declaration order), `__eq__` (field-by-field equality returning `bool`), and `__repr__` (producing `ClassName(field=value, ...)`). Class-level annotated assignments (`x: int`) are accepted as dataclass fields, but only scalar slot types (`int`/`float`/`bool`/`str`, plus a generic type parameter `T` substituted at monomorphization time) are supported — a non-scalar field type (`list[T]`, `dict[K, V]`, `set[T]`, `tuple[...]`, `None`, or a class instance including a self-referential `next: Node`/`next: Self`) is rejected with `C0001` (the attribute-slot storage is a single word per slot, D-154); all other class-body statements are rejected with `C0001` (a class docstring is exempted, [#744](https://github.com/rotnov/pycc/issues/744)). Field defaults, `field(default=...)`, `field(default_factory=...)`, and `@dataclass(frozen=True)` or any other option-bearing form are rejected with `C0001`. Dataclass inheritance merges parent fields (via MRO, parent-before-child order) with the child's own fields; a dataclass inheriting from a non-dataclass base is rejected with `C0001`. Explicit `__init__`, `__eq__`, or `__repr__` definitions in a `@dataclass` body are rejected with `C0001`. `__ne__` is lowered as the negation of the generated `__eq__` call (no separate method is synthesized). `print()` and f-string interpolation of a dataclass instance are rewritten at the MIR level to call the generated `__repr__`. A `print()` argument or an f-string interpolation whose type is any other class instance (a plain class, an `Enum` member, a non-dataclass subclass of a dataclass, a user exception class, or a user class declared under a builtin exception name whatever its shape) or a protocol-typed parameter is rejected by `pycc_types` with `C0001` at `1:1` before MIR runs ([#977](https://github.com/rotnov/pycc/issues/977), [D-237](./decisions/D-237-reject-string-conversion-of-a-non-dataclass-non-exception.md)); the only instance types accepted at those two sites are a `@dataclass` instance (including a monomorphized specialization of a generic `@dataclass`, which keeps the origin's dataclass identity) and a caught builtin exception whose class-table entry is the seeded one (or one of the flat seven when the module shadows a builtin exception name and seeding is withheld). Same-class `==`/`!=` comparisons are accepted when `__eq__` exists (via the MRO); incompatible class comparisons produce `T0021`. PEP 3129 class-decorator syntax is supported as the vehicle for `@dataclass`/`@dataclass_transform()`; other class decorators are rejected with `C0001`. General function-decorator syntax remains unsupported. **[#541 Part 1] The seven builtin exception classes are now real classes** (D-188): HIR lowering synthesizes a `HirClassDef` for `Exception`, `ValueError`, `TypeError`, `KeyError`, `IndexError`, `ZeroDivisionError`, and `RuntimeError`, seeded into `HirModule::class_defs` before any user statement is lowered, so they resolve through the same class table, MRO walk, and annotation projection user classes do. Seeding is gated on the module actually *referencing* one of the seven names (a base class, `raise` operand, `except` type, annotation, `isinstance`/`issubclass` argument, attribute access, or any other spelling at any depth, found with the AST crate's generic visitor): a module that names none of them is seeded with none of them, since it cannot observe the difference and every class-table entry costs per-item lowering and per-function class-binding work. An absent name reads as un-shadowed, exactly as before #541, so `raise`/`except` are unaffected. `Exception` carries a synthetic `__init__(self, message: str)` and the other six inherit it, so `class MyError(ValueError):` linearizes an MRO and `MyError("boom")` resolves the inherited constructor. Attribute access on a bare builtin exception class name (`ValueError.args`) now reports `T0044` rather than the pre-#541 `T0021 name \`ValueError\` is not defined`, since the name resolves to a real class that declares no attribute slots. Instantiating a *synthetic* builtin exception class as a value (`e = ValueError("x")`) is rejected with `C0001` (unchanged by Part 1 -- the diagnostic comes from the callable-builtin check, with a second guard in `class::resolve_instantiation` behind it) -- D-173 propagates a raised exception through global runtime state, not through an allocated instance, so raising is the only supported construction. The synthesis is also all-or-nothing per module: a module whose own top level binds any of the seven names is seeded with none of them, and a user class shadowing one of the names is an ordinary, instantiable class that closes the `except`/`raise` gates against that name exactly as before. Which class table entries count as synthetic is decided by recorded provenance (`HirModule::seeded_builtin_exception_classes`, set by lowering at the point it seeds), never by comparing a `HirClassDef` against the synthesized shape -- a user-authored class can be structurally identical to a synthetic one, so shape is not evidence of origin. **[#541 Part 2] User-defined exception classes can now be raised and caught** (D-189): a user class whose MRO reaches one of the seven builtin exception classes is assigned a runtime type tag from `7..=255` at HIR-lowering time, in module source order, recorded on `HirClassDef::exception_type_tag` (the synthetic builtins carry `None` there and keep their name-resolved tags `0..=6`); a module declaring more than 249 such classes is rejected with `C0001`. `raise MyError("boom")` and `except MyError:` now type-check, and a handler naming a class matches its whole subtree, because the handler carries the sorted set of every raisable class whose MRO reaches it. Four shapes stay rejected. A raisable class that declares its own `__init__`, or inherits one from a non-synthetic ancestor, is `C0001`: the message string is the only payload `PyExceptionObj` carries, so the class's own fields would be silently dropped. "Non-synthetic ancestor" is decided by D-188 provenance, not by how the constructor came to exist, so a hand-written constructor on a user-declared ancestor is `C0001`, reported at the raise operand rather than at either class definition. **[#966] A merely *synthesized* one no longer counts**: the raisability walk ranks #912's implicit constructor last like every other constructor seam ([D-232](./decisions/D-232-rank-an-implicit-object-style-constructor-last-in.md), superseding D-189 rule 4), so `class Base: pass` / `class MyError(Base, Exception): pass` / `raise MyError("boom")` now reaches `Exception.__init__` and **is raisable**, as CPython raises it. The same re-ranking makes binding that class as a value (`m = MyError()`) reach the D-188 synthetic-owner guard and become `C0001` where it previously compiled -- the same rejection every other raisable class already got. `except MyError as e:` is `C0001`, since binding would type `e` as `Ty::Instance` -- read by every consumer as a `PyInstanceObj` -- while the runtime value is a `PyExceptionObj`. And `raise <bound value>` (`e = MyError("x"); raise e`) stays `T0021`: `check_raise_operand`'s new acceptance is keyed structurally on the `HirExpr::Call` shape rather than on the inferred type, precisely because `e` and `MyError("boom")` infer the identical `Ty::Instance("MyError")`, so a type-keyed rule could not separate them and would reinterpret a `PyInstanceObj*` as a `PyExceptionObj*`. And [#714] a fresh construction bound to a name (`e = MyError("boom")`, without ever reaching `raise`) is `C0001` at the earlier construction site itself, closed by `class::resolve_instantiation` keyed on the resolved `__init__`'s owning MRO ancestor being synthetic (`Environment::is_synthetic_class`, provenance-based per D-188, never the mangled name or shape alone) rather than on `raise <bound value>`'s later structural check: `pycc_hir::lower_all` (and so `lower_checked`) gives the synthetic `Exception.__init__` placeholder a real HIR/MIR body only so `raise MyError("boom")`'s own type checking has a function signature to validate against, but codegen binds its function-pointer slot at that placeholder's always-last module position, so any earlier call through the slot -- effectively every real program -- observed a null pointer and aborted at runtime with a `NameError` before this fix. All four wait on Part 3 of #541 ([#703](https://github.com/rotnov/pycc/issues/703)), which materializes a real exception instance.

## Call surface

What a call site may write, and what pycc does with it, is checked in two
places: argument *shapes* are resolved while lowering to HIR (`pycc_hir`),
and argument *types* are checked against the callee's signature afterwards
(`pycc_types`).

Positional arguments are the base case and are unrestricted: `f(1, 2)`
lowers to `HirExpr::Call { callee, args }` unchanged, and `pycc_types`
reports a count mismatch or a type mismatch as `T0021`.

Keyword arguments (`f(b=2, a=1)`, Part 1 of
[#884](https://github.com/rotnov/pycc/issues/884) /
[#1125](https://github.com/rotnov/pycc/issues/1125)) are bound to parameters
*by name during lowering* and replaced by the equivalent complete positional
argument vector, so `HirExpr::Call` stays positional and nothing downstream
of HIR observes the difference. Binding applies to exactly one call shape:
a call whose callee is a bare name that refers to a module-level `def` in
the *same module*, whose parameters are all positional, whose defaults,
if any, are all in the admitted literal subset described below, and whose
name is bound only once in module scope (see "Keyword arguments and default
parameter values on a redefined name" below), and whose binding would
not observably reorder its argument values (see "Keyword argument
evaluation order" below). Every other
shape — a method call, `super().m()`, a container or stdlib-intrinsic call,
a class instantiation, `range(stop=3)`, and `**kwargs` unpacking — keeps the
unchanged `C0001` rejection "keyword call arguments are not supported yet",
because pycc has no signature to bind against there yet.

Default parameter values (`def f(a: int, b: int = 2)`, Part 2 of
[#884](https://github.com/rotnov/pycc/issues/884) /
[#1189](https://github.com/rotnov/pycc/issues/1189)) are filled *during
lowering* through that same binder, so a call that omits a defaulted argument
produces exactly the argument vector the same literal written at the call site
produces, and nothing downstream of HIR observes the difference. A default is
therefore never checked more strictly, or more loosely, than its explicit
twin. Four rules follow from that model:

- **The admitted subset is syntactic.** A default may be a literal `int`,
  `float`, `bool`, `str`, or `None`, optionally with a source-level unary `-`
  or `+` applied to a numeric literal (`= -1`, `= +1.5`), folded exactly as
  the same text is folded at a call site — so `= -9223372036854775808` is
  admitted and `= 99999999999999999999` is not. Anything else — a name, a
  call, a container or f-string literal, a complex literal, a walrus, an
  arithmetic expression — is `C0001` at the default's own span, reported once
  at the `def` however many times the function is called.
- **The scope is a module-level `def`.** A default on a method, a
  `@classmethod`, a `@staticmethod`, or a `Protocol` member keeps the
  unchanged `C0001` "default parameter values are not supported yet", for the
  same reason keyword arguments do there: pycc has no signature to fill from.
  A `@dataclass` field default is its own deferred feature (above), and the
  receiver's own `self`/`cls` default keeps its own message. The binder is
  per module, so an **imported** `def`'s default is not filled either — a
  short call to it is the ordinary `T0021` arity error at the import, while
  a keyword call to it keeps `C0001` like every other unbindable shape.
  Part 3 of #884 widens both to the module boundary. A `def` whose name is
  bound more than once in module scope fills no default either (see "Keyword
  arguments and default parameter values on a redefined name" below).
- **No type is inferred from a default.** An unannotated parameter of a
  private helper keeps its inferred type; the default does not seed it. A
  public function's parameter still needs its annotation (`T0001`).
- **A mismatch is `T0021`, not `T0025`.** The def-site syntax resembles an
  annotated assignment, but by this model the default *is* a call-site
  argument, so it is checked with the call-argument rule and reported with
  the call-argument code. Assignability is the ordinary one: rule 4's `bool`
  as an `int` subtype holds, and D-086 grants no implicit widening, so
  `def f(x: float = 1)` is refused exactly as `f(1)` at a `float` parameter
  is. A PEP 695 type-parameter-annotated parameter (`def f[T](a: T = 1)`) is
  `C0001`: a default is materialized before monomorphization picks `T`.

Positional-only parameters (PEP 570, `def f(a, /, b)`) fill positionally
like any other parameter but can never be named by a keyword, matching
CPython.

Within the bindable shape, the ways a call can still be wrong are errors
CPython itself raises as `TypeError`, so they are `T0021`, not `C0001`:
more positional arguments than the callee has parameters, an unexpected
keyword name, a positional-only parameter passed as a keyword, a parameter
supplied both positionally and by keyword, and a parameter left
unsupplied. Each is reported at the offending keyword's own
source span where one exists, and at the call's span otherwise.

### Keyword argument evaluation order

This is the canonical statement of the rule; other documents cross-reference
it ([#1204](https://github.com/rotnov/pycc/issues/1204)). CPython evaluates
a call's argument values in source order, while the bound positional vector
is evaluated in parameter order. A keyword call in the bindable shape above
is therefore bound only if at least one of these holds:

1. its argument values, taken in source order, already land in
   non-decreasing parameter order, so binding moves nothing; or
2. every argument value of the call, positional and keyword, is a literal in
   the subset a parameter default admits ("The admitted subset is
   syntactic" above) or a bare name, so moving
   one cannot be observed.

Otherwise the call keeps the unchanged `C0001` rejection "keyword call
arguments are not supported yet", byte-identical to every other unbindable
keyword call. So `f(b=g(2), a=g(1))` is `C0001`, while `f(a=g(1), b=g(2))`,
`f(g(1), b=g(2))` and `f(b=2, a=x)` all bind and print CPython's output. A
default filled for an omitted parameter does not count as reordering: it is
a literal, which CPython evaluates once at `def` time. A keyword call the
binder rejects as a `TypeError` (an unexpected, positional-only or
already-supplied name) keeps that `T0021` whatever its order. The rule is
`crates/pycc_hir/src/expr/keyword_bind/eval_order.rs`, and its "cannot be
observed" test is the purity predicate in
`crates/pycc_hir/src/expr/unobservable.rs`, which augmented assignment and
chained assignment share ("Augmented assignment" and "Chained assignment"
below); end-to-end tests:
`tests/issue_1204_keyword_eval_order.rs`.

### Keyword arguments and default parameter values on a redefined name

This is the canonical statement of the rule; other documents cross-reference
it. A name bound more than once in module scope is outside the bindable shape
for both keyword binding and default filling, whichever of its bindings a
call would reach. "More than once" counts two top-level `def`s of the name,
or one `def` plus any other module-scope binding of it: an assignment, an
annotated assignment with a value, an augmented assignment, an `import` or
`from ... import` alias, a `type` alias, a `class`, a walrus, a `match`
capture, a `for`, `with` or `except ... as` target, or a `del`. A binding in
the body of a module-level `if`, `while`, `for`, `try`, `with` or `match`
counts, because that body runs in module scope. A binding inside a `def` or
`class` body does not count, and neither does a comprehension's own target
or an annotation without a value. That exclusion is sound only while pycc
rejects a `global` declaration (`C0001`): once `global` is supported, a
binding it routes to module scope from a function body must count too. The
scan likewise counts a `from ... import *` as binding no named symbol, which
is sound only while pycc rejects a wildcard import (`C0001`); accepting one
must expand it into the names it binds.

The reason is dispatch order. pycc calls a redefined `def` in source order
([#22](https://github.com/rotnov/pycc/issues/22)): a call made before the
second `def` runs the first. Binding, by contrast, happens once per module
against one static table, and a call inside a function body runs at a time
its source position does not fix. So no call site can be tied to one of the
signatures, and binding against either could silently pass the wrong value
or fill the wrong default. Such a name is therefore left out of the table:

- a keyword call to it keeps `C0001` "keyword call arguments are not
  supported yet";
- a call that omits a defaulted argument is left exactly as written and is
  rejected by `pycc_types`' ordinary call checks. For two `def`s of the name
  that is the positional arity check, `T0021` "`f` expects N argument(s),
  got M"; another rebinding shape may be rejected by a different check first,
  but never compiles;
- a call that supplies every argument positionally is unaffected and still
  dispatches in source order.


### Augmented assignment

This is the canonical statement of the rule; other documents cross-reference
it ([#1209](https://github.com/rotnov/pycc/issues/1209), Part 1 of
[#1018](https://github.com/rotnov/pycc/issues/1018)). `target op= value`
behaves exactly as CPython does, or is refused:

1. **(I1)** A name bound to an immutable value is observably identical to
   `target = target op value`.
2. **(I2)** A mutable target, which CPython updates in place through
   `__iop__`, is never admitted (S1 below), so no admitted statement rebinds
   where CPython would mutate.
3. **(I3)** An attribute or subscript target evaluates its container and index
   observably once, in CPython's order: container, index, load, value,
   operation, store.
4. **(I4)** Anything pycc cannot honour exactly is refused with a named
   diagnostic and never miscompiled.

pycc lowers an admitted statement as the plain assignment
`target = target op value` (`crates/pycc_hir/src/stmt/aug_assign.rs`). That
assignment then goes through every check a written-out `target = target op
value` gets. So I1 to I4 hold relative to that plain assignment, which brings
along its own documented deviations and adds none of its own:

- a negative buffer index raises instead of wrapping (D-108, `docs/RUNTIME.md`'s
  `memoryview` row);
- `int ** negative` is [#1068](https://github.com/rotnov/pycc/issues/1068);
- a bigint `*`, `//`, `%` or `**` raises `OverflowError`
  ([#1040](https://github.com/rotnov/pycc/issues/1040));
- a `dict` value past the inline-integer range aborts
  ([#1089](https://github.com/rotnov/pycc/issues/1089));
- `<<` raises `OverflowError` where CPython raises `MemoryError` (rule 8).

One imprecision is inherited rather than a deviation: CPython's `d |= pairs`
on a `dict` accepts any iterable of key-value pairs, but its plain form
`d | [...]` is a `TypeError`, so `d |= [("a", 1)]` is refused with the plain
form's `T0021` "not defined" instead of being admitted.

The admitted operators are exactly the plain binary operators
(`crates/pycc_hir/src/expr/bin_op_kind.rs`, the one mapping both paths use):
`+= -= *= /= //= %= **= <<= >>= &= |= ^=` (the last five since
[#1210](https://github.com/rotnov/pycc/issues/1210), Part 2 of #1018, typed by
rule 8 above). The one other operator, `@=`, is `C0001` "augmented assignment
operator `@=` is not supported yet", as the plain `@` is "binary operator `@`
is not supported yet". The admitted target shapes are:

- a name;
- an attribute of a name (`self.n`, `obj.n`);
- a subscript of a name whose index is a name or a literal. The literal subset
  is the one a parameter default admits, and the purity predicate
  `is_unobservable` in `crates/pycc_hir/src/expr/unobservable.rs` decides it.

Each other shape has its own `C0001`:

- an attribute or subscript of any other expression: "... of a computed
  expression is not supported yet; bind the object to a name first";
- a slice: "augmented assignment to a slice is not supported yet";
- any other index: "augmented assignment with a computed index is not
  supported yet; bind the index to a name first".

Every type refusal is the one the plain assignment gets. So `self.n += 1` in
`__init__` before any `self.n = ...` is `T0044`, as is a class-level attribute
or a getter-only property. A name the function binds only through `+=` is the
unbound-local `T0021`, and a `bool` name is `T0023` (it cannot hold the `int`
the operation produces) unless the operator is `&=`, `|=` or `^=` and the value
is a `bool`, which keeps the name `bool`. An `Enum` member's `value`
or `name` is `T0044` too ([#1219](https://github.com/rotnov/pycc/issues/1219)).

The load and the store each read the container and the index. That second
read cannot be observed, and I2 holds, only under these conditions:

- **(S1)** It is sound only while binary-operator type checking admits only
  the immutable operand types `int`, `bool`, `float` and `str`. `xs += [1]`,
  `t += (2,)` and a user class operand are all `T0021` today. A change that
  admits a mutable operand to `+` (list concatenation,
  [#1017](https://github.com/rotnov/pycc/issues/1017)) or a user operator
  dunder must give augmented assignment a real in-place path for that type,
  instead of this rewrite.
- **(S2)** It is sound only while pycc rejects `global` and `nonlocal`
  (`C0001`), so no call inside the value can rebind a local container or
  index between the two reads.
- **(S3)** It is sound only while a walrus in an assignment's value is refused
  by the placement check (#774), so the value cannot rebind them either.
- **(S4)** It is sound only while the index predicate is `is_unobservable`.
  Widening it to computed expressions needs its own argument that no admitted
  index can raise or be observed, which `int // 0` and a bigint allocation
  would fail.

A call inside the value can still mutate the object the container name
refers to, but not change which object it is, and CPython stores into that
same object. An attribute that cannot be written today is also not
expressible today: `@dataclass(frozen=True)`, `NamedTuple` and `__slots__`.
The rule is sound only while that holds, and a change that admits one must
refuse writes to it. End-to-end tests are in `tests/issue_1018_aug_assign.rs`.
The byte-exact oracle fixtures are `tests/fixtures/aug_assign_scalars.py` and
`tests/fixtures/aug_assign_targets.py`.


### Chained assignment

This is the canonical statement of the rule; other documents cross-reference
it ([#1213](https://github.com/rotnov/pycc/issues/1213), Part 5 of
[#1018](https://github.com/rotnov/pycc/issues/1018)). For
`t1 = t2 = ... = tn = e`, CPython evaluates `e` once, then assigns it to `t1`,
`t2`, ... `tn`, left to right. Each target's own base and key are evaluated
when that target is assigned, so in `n = d[f"{n}"] = 5` the key reads the `n`
the first target just bound.

pycc rewrites the chain into single-target assignments
(`crates/pycc_hir/src/stmt/chain_assign.rs`) and lowers each one as the
written-out assignment. Every check and refusal of a single-target assignment
therefore applies to each target unchanged:

- When `e` is `is_unobservable` (a name, or a literal in the subset a parameter
  default admits), each target is assigned its own copy of `e`:
  `a = b = 0` becomes `a = 0; b = 0`. Reading such an `e` again cannot be
  observed. A name target equal to `e` rebinds it to the value it already
  holds. The copied literals are immutable, and their identity is not
  observable only because pycc refuses `is` (other than against `None`) and
  `id()` today. A change that admits either must bind the value once instead.
  Likewise, a copied name reads the same value each time only because no
  later target's own evaluation can rebind it: pycc refuses `global` and
  `nonlocal`, so a call in a later target's base or key cannot reach the
  name. A change that admits either must revisit this path the same way.
- Otherwise `e` is bound once to a synthesized temporary, `0chain_<offset>`
  (`<offset>` is the statement's byte offset), and each target is assigned
  from it. The temporary's leading digit means no source name can equal it,
  the same D-117 argument comprehension variables use. `a = b = [1]` makes
  `a` and `b` the same list, as in CPython.

A chain is refused, with `C0001`, when:

- `e` is an empty `[]` or `{}`, because the empty-container pass infers the
  element type from uses of the same name and could only name the temporary.
  The message is "chained assignment of an empty `[]`/`{}` literal is not
  supported yet; inside a function, annotate one name and assign it
  (`a: list[int] = []`, then `b = a`)".
- a target is a tuple, list or starred target, which gets the single-target
  refusal "only assigning to a bare name is supported so far, got a tuple"
  ([#891](https://github.com/rotnov/pycc/issues/891));
- the chain is in a class body ("a class-level attribute assignment must have a
  single target") or an enum body, which keep their own refusals.

`self.x = self.y = v` in `__init__` declares both attributes, each typed from
`v`. A subscript target has the single-target limits: only a `dict[str, int]`
or buffer store compiles, and `list[int]` item assignment is `T0033`.

Two known limits, neither of which produces wrong output:

- **Module temporaries.** A program's modules share one namespace, so two
  modules whose module-level chains, with non-trivial values, start at the same
  byte offset share one `0chain_<offset>` global. The temporary is written
  immediately before it is read, so sharing it is harmless when both values
  have the same type. When they differ, the program is refused or its slot
  type is widened. The temporary is not recorded as a definition, so it never
  causes the cross-module name collision `C0001`.
- **Buffer allocations.** Under `pycc build --ext`, `a = b = ndarray(n)` binds
  the allocation to the temporary. The targets then read a buffer-bound name,
  so the build is refused with the `C0001` that begins "using `{name}`, which
  is bound to buffer storage this `pycc build --ext` artifact allocated", and
  that message names `0chain_<offset>`. Allocate into one name instead.

End-to-end tests are in `tests/issue_1213_chain_assign.rs`, and the byte-exact
oracle fixture is `tests/fixtures/chain_assign.py`.

### `and` and `or`

This is the canonical statement of the rule; other documents cross-reference
it ([#1211](https://github.com/rotnov/pycc/issues/1211), Part 3 of
[#1018](https://github.com/rotnov/pycc/issues/1018)). `a and b` and `a or b`
return one of their operands, as in CPython, and short-circuit: `a` is
evaluated exactly once, `b` at most once and only when `a` does not decide the
result, and each operand's truth is tested at most once. A chain
`a or b or c` is right-folded (`HirExpr::BoolOp`).

**Two contexts.** HIR lowering marks a `BoolOp` as *truth context* when its
value is consumed only for its truth: an `if`/`elif`/`while` test, a
comprehension `if` filter, and the operand of `not`. The marking descends only
through a `BoolOp`'s own operands and through `not`; it does not descend
through a walrus (`if (x := a or b):` binds the selected value), a comparison,
a call or any other node. A `match` guard stays value context, since the
checker requires a guard to be `bool`. Every other position is value context.

- In truth context the result is `bool`, and each operand needs only to be
  truth-testable; operand types need not agree (`if n > 0 and name:` with an
  `int` and a `str` compiles).
- In value context the result type is the join `J` of the two operand types,
  computed by one function, `pycc_hir::bool_op_result_ty`, that the checker
  and MIR lowering share. Under `or` a left `Optional[T]` is first read as
  `T`, because the left operand is selected only when it is truthy, and so
  present.

| Left (after the `or` rule), right | Result |
|---|---|
| equal: `bool`, `int`, `float`, `str`, `Optional[int\|float\|bool]`, or the same class instance | that type |
| `bool` and `int`, either order | `int` (a selected `bool` keeps its identity: `True or 0` prints `True`) |
| `T` and `Optional[T]`, either order | `Optional[T]` |
| anything else | `T0021` "`or` operands have no common type: int and str (pycc has no union types)" |

So `x or default` with `x: int | None` and `default: int` is an `int`. The
`int`/`float` pair is refused, not widened: `1 or 2.0` is the `int` `1` in
CPython, and a widened `1.0` would print differently.

**Admitted operands.** In both contexts an operand must be truth-testable:
`bool`, `int`, `float`, `str`, `None`, `Optional[int|float|bool]` or a class
instance (`pycc_types::unop::is_truth_testable`, the same set `not` admits).
A container, `memoryview` or `Protocol`-typed operand is `T0021` ("`and`
operand of type `list[int]` has no truth value pycc can test"). A class
instance whose class or any base class defines `__bool__` or `__len__` is
`T0021` too, naming the dunder, because pycc does not call either for a truth
test yet. A `None` operand is admitted in truth context only. A CPython-object
operand is `I0404`: `if obj:` is admitted, but a foreign object has no value
join yet.

**Walrus.** A walrus in the first operand always executes and is admitted. A
walrus anywhere in a later operand would bind only conditionally, which the
function-wide binding walkers do not model, so it is refused at HIR lowering
with `C0001` "a walrus assignment (`:=`) in a short-circuited `and`/`or`
operand is not supported".

**No narrowing.** A test inside `and`/`or` does not narrow:
`x is not None and x > 0` is still `T0021` on the right operand. This is the
D-205 scope cut described under "Narrowing & flow typing" above.

**Ownership.** An `int` or `str` result is always owned: an operand that is a
duplicate reference (a name, an attribute read) is retained or incref'd in its
own arm before any coercion, and the extracted payload of a left
`Optional[int]` under `or` is always retained. The discarded left operand is
released before the right one is evaluated, so a right operand that raises
leaks nothing. In truth context each operand is reduced to a bit and its
`int` temporary is released at once, as `not` does. Three leaks are accepted
and bounded: a discarded `str` temporary (as in an `if` test today), the
always-retained `Optional[int]` payload when the left operand is an owned
temporary, and a discarded owned `Optional[int]` left operand. An
`Optional[int]` result copies its arm exactly as `z = y` does.

End-to-end tests are in `tests/issue_1211_bool_ops.rs`, and the byte-exact
oracle fixture is `tests/fixtures/bool_ops.py`.

### Chained comparisons

This is the canonical statement of the rule; other documents cross-reference
it ([#1212](https://github.com/rotnov/pycc/issues/1212), Part 4 of
[#1018](https://github.com/rotnov/pycc/issues/1018)). `a < b < c` means
`a < b and b < c`, as in CPython, except that `b` is evaluated once. A chain
of two or more operators lowers to one `HirExpr::CompareChain`; a single
comparison stays `HirExpr::Compare`.

- **Per-link admission.** Each link is admitted and typed exactly as a single
  comparison of its two operands: `==`, `!=`, `<`, `<=`, `>`, `>=` over the
  numeric/`bool`/`str` pairs a single comparison accepts, `==`/`!=` between
  two instances of the same dataclass, and `is`/`is not` only when one of
  *that link's* two operands is the literal `None` (D-197). A bad link is the
  single comparison's `T0021` "cannot compare `int` and `str`". `in`/`not in`
  anywhere in a chain keep their `C0001` "comparison operator not supported
  yet".
- **Evaluation.** Operands are evaluated left to right, each at most once.
  The chain stops at the first false link, so no later operand runs. The
  result is always `bool`.
- **Walrus.** Operands 0 and 1 always run, so a walrus there is admitted. A
  walrus in operand 2 or later would bind only conditionally and is refused at
  HIR lowering with `C0001` "a walrus assignment (`:=`) in a short-circuited
  chained-comparison operand is not supported", the rule `and`/`or` applies
  to its later operands.
- **No narrowing.** A chain is not a narrowing test: `if 0 < x is not None:`
  does not narrow `x`.
- **Exceptions and ownership.** A link comparing a heap bigint raises pycc's
  `OverflowError` (the Language-surface row's bigint-comparison gap in
  `docs/ROADMAP.md`); the chain checks for it after the link, so no later
  operand runs. Each `int` operand temporary is released exactly once on
  every path, including that exception edge.

End-to-end tests are in `tests/issue_1212_chained_compare.rs`, and the
byte-exact oracle fixture is `tests/fixtures/chained_compare.py`.


### `del` statement

This is the canonical statement of the rule; other documents cross-reference
it ([#1244](https://github.com/rotnov/pycc/issues/1244), Part 1 of
[#1216](https://github.com/rotnov/pycc/issues/1216)). `del a, (b, [c])`
deletes `a`, `b` and `c`, left to right, and `del ()` deletes nothing, as in
CPython. HIR expands the statement into one `HirStmt::Delete` per name
(`crates/pycc_hir/src/stmt/del.rs`). Under D-124's leak-only model, MIR lowers
each one to a no-op: nothing is released, and the checker alone guarantees
that no read follows the deletion.

**Binding state.** A `del x` needs `x` to be `Definitely` bound. A `Maybe`
binding is `T0041` and no binding at all is `T0021`, the same diagnostics a
read gets, so `del a, a` and `del len` are refused. After the `del`, `x` is
`Maybe` bound with its old type, and its narrowing is dropped. A later read is
therefore `T0041`, and so is a read after an `if` whose one arm deleted `x`.
A rebinding makes `x` `Definitely` bound again. The sticky representation
(D-040) survives the deletion, so `del x; x = "s"` after an `int` binding is
`T0023`.

**Prescans.** A loop or handler body can run after a `del` in itself or in an
earlier body, so a deletion anywhere in such a body demotes `x` to `Maybe`
before the body is checked (`narrow::apply_delete_prescan`):

- every `while` body, and every `for` body over `range`, a list, an `enum` or
  a CPython iterable (the loop's own target is exempt: each iteration rebinds
  it);
- the state after a loop: a name the body deleted stays `Maybe`, even when the
  body rebinds it later;
- a `try` handler, after a `del` in the `try` body; an `except*` handler,
  after a `del` in the body or an earlier handler;
- the code after a `try` statement that has a non-empty `finally`, after a
  `del` in any of its bodies.

**Refused, with `C0001`:**

- a target that is not a bare name: an attribute (`del o.a`), a subscript
  (`del d[k]`, `del xs[i]`, tracked in
  [#1245](https://github.com/rotnov/pycc/issues/1245) and
  [#1246](https://github.com/rotnov/pycc/issues/1246)) or a slice;
- `del __name__`, and a `del` in a class body;
- a `del` of a method's receiver (`self`, a renamed receiver, or `cls`),
  because the zero-argument `super()` reads the receiver slot. A
  `@staticmethod` has no receiver, so `del self` compiles there;
- a `del` of a function or class name (including a builtin class such as
  `ValueError`), of a `Final` name, of a buffer the function releases on
  return, or of a name holding a CPython object, whose release could run a
  foreign finalizer (`pycc_types/src/del_stmt.rs`). Like every `pycc_types`
  diagnostic, these render at `1:1`, not at the `del` (D-043);
- at module scope, a `del` of an imported name (`import m`, `from m import x`);
- at module scope, a `del x` while any `def` or `class` of the module mentions
  `x`. Those bodies are checked after all top-level code (D-041), so the
  checker could not see a call after the `del` that reads `x`;
- at module scope, a `del x` while another module of the program mentions
  `x`, because linked modules share one top-level namespace
  (`program::link`), and a `from m import x` when `m`'s top level deletes `x`.

Every module-scope rule is conservative: a function that only binds its own
local `x` still blocks `del x` at module scope.

Known limits, each a refusal rather than wrong output:

- a name deleted and rebound in a loop body, or in an earlier `except*`
  handler, is `Maybe` bound after it;
- a name deleted and rebound inside a `try` with a `finally` is `Maybe` bound
  after the statement;
- `if c: del x; return` still leaves `x` `Maybe` bound after the `if`, and a
  `for` over `range(0)` or a walrus-driven `while` still counts its body's
  deletions;
- a local that shadows a module-level function or class name cannot be
  deleted.

End-to-end tests are in `tests/issue_1244_del_name.rs`, and the byte-exact
oracle fixture is `tests/fixtures/del_name.py`.

### Comprehensions

This is the canonical statement of the rule
([D-250](./decisions/D-250-comprehensions-are-expressions-with-a-node-scoped-loop-variable.md),
[#1254](https://github.com/rotnov/pycc/issues/1254), Part 1 of
[#1214](https://github.com/rotnov/pycc/issues/1214)). A list, set or dict
comprehension is an expression and may appear anywhere an expression may: a
call argument, a `return` value, an annotated assignment, an f-string, a
`while` or `if` test, an operand of `and`/`or`, or another comprehension. It
runs where CPython runs it, so it is skipped in an untaken short-circuit
operand and keeps its place among its sibling arguments. `name = <comp>`
still lowers to D-117's statement form; both forms check and emit the same
way.

**Scoping.** The loop variable is renamed to D-117's synthesized
`0comp_<offset>_<name>` and bound only inside the comprehension, so it never
leaks: `x = 100; print(len([x for x in range(3)]), x)` prints `3 100`. The
iterable is checked in the enclosing scope, so in a nested
`[len([x for x in range(x)]) for x in range(4)]` the inner `range(x)` reads
the outer loop variable, as in CPython. The element and filter are checked in
a scope that adds the loop variable to the enclosing one, so narrowing and
module globals carry over.

**Types.** The iterable is `range(...)` or a bare name of type `list[T]`,
`set[T]` or `dict[K, V]` (the loop variable is `T` or the key `K`); a bare
name of any other type is `T0033`. The result is `list[int]`, `set[int]` or `dict[str, int]`,
with the element gates of the matching display (`T0034`, `T0038`, `T0036`,
D-119). An unannotated private helper may take or return a comprehension, and
its container type is inferred.

**Refused, with `C0001`:**

- a walrus (`:=`) anywhere inside a comprehension, in either form: "a walrus
  assignment (`:=`) inside a comprehension is not supported yet", spanning the
  comprehension;
- more than one `for` clause (tracked by #1257), more than one `if` filter
  (Part 3 of #1214, #1256), a target that is not a bare name, and `async`
  comprehensions (neither of the last two is tracked yet). Part 2 of #1214
  (#1255) owns the iterable limit above.

**Across modules.** A synthesized name is not a definition, so it never
collides in the link step ([#1237](https://github.com/rotnov/pycc/issues/1237)).
The linked program is one namespace, though, so two modules whose
statement-form comprehensions have the same target name at the same byte
offset share one `0comp_` slot. That works when the loop variables have the
same type and is refused with `T0023` when they do not; #1237 tracks it.

End-to-end tests are in `tests/issue_1214_comprehension_expr.rs`, and the
byte-exact oracle fixture is `tests/fixtures/comprehension_expr.py`.

## Error philosophy

Rust-grade messages: primary span + labels, expected/found diff, suggestion machine-applicable where safe (a planned `pycc check --fix` flag would apply trivial ones once implemented; not yet implemented, see `docs/CLI_SPEC.md`), `pycc explain T0021` long-form. Every diagnostic documented + tested. Full registry: [DIAGNOSTICS.md](./DIAGNOSTICS.md).
