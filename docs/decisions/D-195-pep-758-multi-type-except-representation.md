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

  For the `as`-binding representative type, when more than one name is
  present: bind to the **first-listed type in source order** (`names[0]`).
  All four binding sites call one shared helper,
  `pycc_hir::except_handler_binding_type_name`, rather than inlining
  `names[0]` four times independently — the one place this choice is made
  is the one place it can drift out of agreement across sites. MIR's own
  handler-tag computation is a separate, unrelated union: it unions every
  named type's `handler_type_tags` result (not just the first) and sorts +
  dedups the combined set, since two named types can share tags through a
  common ancestor (e.g. `OSError` and `ConnectionError` both include tags
  `10` and `19..=22` — `ConnectionError`'s own tag and its four children
  are already inside `OSError`'s tag set) and a naive concatenation would
  double-count.
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
    MRO-containment scan), so this would add a new tree operation whose
    only consumer is a binding that Part 3 of #541 already documents as not
    materializing a real, readable exception instance for user-defined
    classes — the bound value's *declared* type is not yet observable
    through any operation more refined than `raise e`, so a more precise
    ancestor type buys no current capability. First-listed is exactly as
    correct for every operation the bound value currently supports
    (re-raising it), is trivial to compute, and is deterministic from
    source text alone rather than from tree shape.
  - **The `as` binding's representative type is always the literal
    `"Exception"`.** Rejected: it is strictly less precise than
    first-listed for no simplicity gain — `except_handler_binding_type_name`
    is exactly as cheap to call as a hardcoded literal — and it would
    silently widen the bound value's `Ty::Instance` for every multi-type
    handler with an `as` binding, including the common case of a
    single-element list (`except (A,) as e:`, which must behave exactly
    like `except A as e:`).
- Consequences: `HirExceptHandler.exc_type`'s never-empty invariant on
  `Some` is documented but not compiler-enforced (no dedicated non-empty
  vector type is introduced, deliberately out of scope for this issue) — a
  future change to `lower_except_handler` that constructs a `Some(vec![])`
  by mistake would violate the invariant silently at the type level, though
  MIR's own `handler_type_tags` call would still panic loudly the moment
  such a handler reached MIR, rather than let it through. The
  first-listed-name binding choice is now load-bearing at all four binding
  sites through the one shared helper; a hypothetical future feature that
  needs a different binding rule (e.g. once Part 3 of #541 materializes
  real per-class exception instances and per-name attribute access on the
  bound value becomes meaningful) would revisit this helper's one
  definition rather than four independent call sites.
