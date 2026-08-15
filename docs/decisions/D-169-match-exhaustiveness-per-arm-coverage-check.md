---
id: D-169
title: "Match exhaustiveness: per-arm coverage check instead of decision-tree compilation"
status: accepted
---

## D-169: Match exhaustiveness: per-arm coverage check instead of decision-tree compilation
- Status: accepted
- Context: PEP 634 structural pattern matching (`match`/`case`) requires an exhaustiveness check to warn or error when a `match` statement does not cover every possible value of the subject's type. The full PEP 634 specification describes a decision-tree compilation algorithm that tracks which sub-patterns are refutable and uses a usefulness check to determine whether a given arm adds coverage. That algorithm is complex, requires a notion of "specialized" patterns, and is difficult to implement correctly for all pattern forms (sequences with star captures, mappings with rest captures, class patterns with inheritance, or-patterns, guards).

- Decision: pycc uses a simpler per-arm coverage algorithm (D-169) instead of decision-tree compilation:

  1. An unguarded irrefutable pattern (`_`, a capture, or an `Or` containing one) is exhaustive.
  2. A `bool` subject is exhaustive when both `True` and `False` are covered (by singleton patterns or literal `1`/`0`).
  3. An enum subject is exhaustive when all enum members are covered (by class patterns like `Color.RED` or literal patterns matching member values).
  4. All other types require an irrefutable pattern (wildcard or capture) to be exhaustive.
  5. Non-exhaustive matches emit `T0030` and are rejected by the type checker.

  This algorithm is sound (never reports a non-exhaustive match as exhaustive) but incomplete (may report some matches as non-exhaustive that a full decision-tree algorithm would accept — e.g. `match (x, y): case (0, 0): ... case (0, 1): ... case (1, _): ...` is exhaustive for `tuple[int, int]` but the per-arm algorithm does not track positional coverage). The incompleteness is acceptable for v0.3 because the user can always add `case _:` to silence the diagnostic.

- Alternatives:
  - **Full decision-tree compilation (PEP 634 §3):** tracks pattern usefulness and specialization. Rejected for v0.3 because the implementation complexity is disproportionate to the benefit — pycc's type system is already statically resolved (no runtime type tests), so the decision tree's main advantage (generating optimal branching) is less valuable than in a JIT compiler. The per-arm algorithm is ~50 lines of code vs. an estimated ~500+ for a correct decision-tree implementation.
  - **No exhaustiveness check at all:** rejected because `T0030` is already reserved in the diagnostic registry and the plan requires it. Silent acceptance of non-exhaustive matches would be a regression vs. CPython's own `MatchError` runtime behavior.

- Consequences:
  - Easier: the per-arm algorithm is simple to implement, test, and maintain. It covers the most common cases (wildcard, bool, enum) correctly.
  - Harder: users must add `case _:` for complex exhaustive matches that the algorithm cannot prove (e.g. overlapping positional patterns in tuple/class patterns). This is a documentation burden, not a correctness issue.
  - Irreversible: the `T0030` diagnostic semantics are now part of the public diagnostic contract. A future upgrade to decision-tree compilation would need to preserve the same code and summary.
