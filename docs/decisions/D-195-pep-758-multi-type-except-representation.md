---
id: D-195
title: "PEP 758 multi-type except handler representation and binding choice"
status: accepted
---

## D-195: PEP 758 multi-type except handler representation and binding choice
- Status: accepted
- Context: Part 3 of [#543](https://github.com/rotnov/pycc/issues/543)
  ([#740](https://github.com/rotnov/pycc/issues/740)) implements PEP 758:
  `except A, B:` (bare comma, no parentheses) alongside the pre-existing
  `except (A, B):` parenthesized form, both naming more than one exception
  type in a single handler. `HirExceptHandler.exc_type` was
  `Option<String>` — `None` for bare `except:`, `Some(name)` for exactly
  one named type. A multi-type handler needs to carry more than one name,
  and every one of the four sites that read `exc_type` to bind an `as`
  name (MIR lowering, the type checker's own binding, `monomorphize.rs`'s
  generic-call rewriting, `constraints.rs`'s constraint collection) needs
  to agree on which single type that binding gets, since Python's `as`
  binds one name to one value regardless of how many types the handler
  matched against. Two questions this decision answers: how the handler's
  type list is represented in `HirExceptHandler`, and which named type the
  `as` binding uses when more than one is present.
- Decision: widen `HirExceptHandler.exc_type` from `Option<String>` to
  `Option<Vec<String>>`. `None` stays bare `except:`; `Some(names)` names
  one or more types in source order, and `names` is documented as *never
  empty* — enforced at the one production site
  (`crates/pycc_hir/src/stmt/exception.rs::lower_except_handler`), so every
  downstream consumer (MIR, the type checker, `monomorphize.rs`,
  `constraints.rs`) may assume non-emptiness without re-checking. Both
  surface spellings lower to the same `Expr::Tuple` AST shape (only the
  HIR-discarded `parenthesized` flag differs), so `lower_except_handler`
  gained one new match arm: `Expr::Tuple` walks `elts`, rejects an empty
  list, rejects any non-`Expr::Name` element, and collects the names
  preserving source order; the pre-existing bare-`Expr::Name` branch stays
  a separate `Some(vec![name])` case, since a bare name is a different
  `Expr` variant, not reachable through the tuple path.

  `except ():` parses successfully as `Expr::Tuple { elts: [], .. }` —
  syntactically valid, but a handler naming zero exception types. Left
  unchecked, it would reach `pycc_mir::exception::handler_type_tags`'s
  `.expect("pycc_types rejects unknown exception handler types before
  MIR")` and `pycc_codegen::exception`'s
  `accumulated.expect("pycc_mir never emits an empty handler tag set")`
  with an empty tag vector, since MIR would union zero named types' tags.
  It is rejected at HIR lowering with a new diagnostic, `C0001`: "an
  `except` handler must name at least one exception type" — the same
  `C0001` catch-all code the rest of `pycc_hir` already reuses for a
  recognized-but-unsupported statement shape, not a new diagnostic code.

  For the `as`-binding representative type: a **single-name** handler
  (`names.len() == 1`, which includes both `except (A,) as e:` and
  bare-comma `except A,:`) binds to that exact name, matching plain
  `except A as e:`. A **genuinely multi-type** handler
  (`names.len() > 1`) binds to the literal **`"Exception"`**, the
  universal builtin root, rather than to the first-listed type. All four
  binding sites call one shared helper,
  `pycc_hir::except_handler_binding_type_name`, rather than inlining this
  choice four times independently — the one place it is made is the one
  place it can drift out of agreement across sites. MIR's own handler-tag
  computation is a separate, unrelated union: it unions every named type's
  `handler_type_tags` result (not just one) and sorts + dedups the
  combined set, since two named types can share tags through a common
  ancestor (e.g. `OSError` and `ConnectionError` both include tags `10`
  and `19..=22` — `ConnectionError`'s own tag and its four children are
  already inside `OSError`'s tag set) and a naive concatenation would
  double-count. This binding-type choice is unrelated to and does not
  change the handler's actual runtime dispatch, which is driven entirely
  by the tag set, not by the `as` binding's static type.

  **Revision (2026-08-24):** the representative-binding choice originally
  shipped in this same pull request as unconditional first-listed-name
  binding (`names[0]`, for both single- and multi-type handlers alike).
  `chatgpt-codex-connector[bot]`'s review of PR #743 found this unsound,
  not merely imprecise: `isinstance()` in pycc folds entirely at compile
  time from the object's *static* `Ty::Instance` type's MRO
  (`pycc_mir::class::lower_isinstance`,
  `pycc_hir::typecheck::eval_isinstance_single`), with no runtime type
  check backing it up. Binding `except (ValueError, TypeError) as e:` to
  `"ValueError"` made `isinstance(e, ValueError)` fold to a compile-time
  constant `True` even when the handler actually caught a `TypeError` — a
  false *positive*. That is strictly worse than the pre-existing, already-
  accepted imprecision for single-type handlers, which can only produce
  false *negatives* on subclass-narrowing `isinstance()` queries (a single
  declared type is always a genuine ancestor of anything that handler can
  actually catch, so `isinstance()` against that exact type is always
  sound; only a *more specific* subclass query can under-approximate).
  This finding was resolved within the same PR before merge, per this
  decision's own original "consequences" clause anticipating that "a
  hypothetical future feature… would revisit this helper's one
  definition": the review found the need sooner than expected, but the
  mechanism anticipated it correctly. The fix, and the reasoning for why
  `"Exception"` closes the gap (its MRO contains no descendant name, so a
  specific-type query on it can only under-approximate, never claim a
  wrong specific type) is recorded in
  `except_handler_binding_type_name`'s own doc comment in
  `crates/pycc_hir/src/exception.rs`, and pinned by
  `tests/issue_740_multi_type_except.rs`'s
  `isinstance_on_a_multi_type_handler_binding_never_produces_a_false_positive`.
  `raise e` (bare-name re-raise) is unaffected by this choice either way:
  it lowers to `MirExceptionValue::Existing`, which preserves the actual
  runtime exception object and tag rather than reconstructing from the
  static binding type (`pycc_mir::exception::lower_exception_value`),
  confirmed by the pre-existing, still-passing
  `parenthesized_handler_as_binding_reraises_successfully` test.
- Alternatives:
  - **`Vec<String>` alone, dropping the `Option`, with an empty `Vec`
    meaning bare `except:`.** Rejected: it conflates "no type list" (bare
    `except:`, syntactically distinct) with "an invalid empty list"
    (`except ():`, syntactically valid but semantically empty) into the
    same representation, forcing every consumer to re-derive which case it
    is looking at from a `Vec`'s length rather than reading it directly off
    the `Option`'s tag. Keeping `Option<Vec<String>>` with a documented
    never-empty invariant on the `Some` case keeps both distinctions
    explicit at the type level.
  - **The `as` binding's representative type is the nearest common
    ancestor of the named types.** Rejected: pycc does not compute a
    common-ancestor type over the builtin/user exception tree today (only
    membership and subclass-reachability, via `handler_type_tags`'s
    MRO-containment scan), so this would add a new tree operation. Unlike
    the originally-rejected first-listed-name choice, a nearest-common-
    ancestor type would also have been *isinstance*-sound (it is a genuine
    ancestor of every type the handler can catch, so no specific-type
    query against it can produce a false positive) — but `"Exception"` is
    always a valid nearest-common-ancestor upper bound in pycc's tree (a
    single root), is trivial to compute (no tree walk, no new operation),
    and is exactly as sound for the one refinement operation the bound
    value currently supports (`isinstance()`/`raise e`), so the extra
    precision a real nearest-common-ancestor computation would buy has no
    current observable consumer.
  - **First-listed name, unconditionally (`names[0]`), for both single-
    and multi-type handlers.** This was this decision's original choice
    and shipped briefly within this same PR before review. Rejected on
    revision: see the "Revision (2026-08-24)" paragraph above — it is
    `isinstance()`-unsound for a multi-type handler, not just imprecise.
    Kept unconditionally for a *single*-name handler, where it coincides
    with binding to the handler's own one exact type and introduces no
    imprecision at all.
- Consequences: `HirExceptHandler.exc_type`'s never-empty invariant on
  `Some` is documented but not compiler-enforced (no dedicated non-empty
  vector type is introduced, deliberately out of scope for this issue) — a
  future change to `lower_except_handler` that constructs a `Some(vec![])`
  by mistake would violate the invariant silently at the type level, though
  MIR's own `handler_type_tags` call would still panic loudly the moment
  such a handler reached MIR, rather than let it through. The
  binding-type choice (exact name for one type, `"Exception"` for more
  than one) is now load-bearing at all four binding sites through the one
  shared helper; a hypothetical future feature that needs a different
  binding rule (e.g. once Part 3 of #541 materializes real per-class
  exception instances and per-name attribute access on the bound value
  becomes meaningful, or if pycc ever adds a real union/sum instance type)
  would revisit this helper's one definition rather than four independent
  call sites — as this revision itself already did once.
