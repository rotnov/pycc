---
id: D-196
title: "PEP 681 is a second itemized row permanently blocked from flipping, alongside PEP 487"
status: accepted
---

## D-196: PEP 681 is a second itemized row permanently blocked from flipping, alongside PEP 487
- Status: accepted
- Context: [#732](https://github.com/rotnov/pycc/issues/732) found that
  `docs/ROADMAP.md`'s v0.3 Accept clause requires 39 distinct PEP numbers
  ([D-153](./D-153-correct-v0-3-s-conformance-target-before-any-v0.md)) but
  the matrix currently encompasses 34 — a 5-PEP gap that was untracked by any
  checker or issue. While correcting that (teaching
  `scripts/check_conformance_breadth.py` to compute and validate the
  distinct-PEP figure), the issue also asked whether `docs/ROADMAP.md`'s
  existing claim that PEP 681's row "cannot flip on byte-for-byte conformance
  evidence" is actually true, since the paragraph cited
  [#248](https://github.com/rotnov/pycc/issues/248) for it — and #248 is
  closed and never mentions PEP 681 anywhere in its body or comments. The
  claim needed either a real citation or independent verification.

  Verification found the claim correct, but for a mechanism the stale
  citation never actually described. pycc treats `@dataclass_transform()` as
  a no-op synonym for `@dataclass`: `tests/fixtures/pep_0681_dc_transform.py`
  (misfiled — the matrix row at `docs/PYTHON_STANDARDS.md:291` cites a
  nonexistent `py311/pep_0681_dc_transform.py` path; the real file has no
  `py311/` prefix and is not registered in `tests/conformance.rs` at all, so
  nothing runs it today) instantiates `Color(255, 128, 0)` against a class
  decorated only with `@dataclass_transform()`, expecting pycc's synthesized
  `__init__`/`__eq__`/`__repr__` to behave like a real dataclass.

  Real CPython does not synthesize anything from `@dataclass_transform()`
  alone — it is a `typing` marker consumed by static type checkers, with zero
  runtime effect. Reproduced directly:

  ```python
  from typing import dataclass_transform

  @dataclass_transform()
  class Color:
      r: int
      g: int
      b: int

  c1 = Color(255, 128, 0)
  ```

  Observed on CPython 3.14.6 (the closest 3.14.x patch available in this
  environment to the project's pinned oracle, CPython 3.14.7 — the same
  minor version, one patch release apart, and `dataclass_transform`'s runtime
  no-op behavior has no patch-level changelog entry between them):

  ```
  Traceback (most recent call last):
    File "dc_transform_check.py", line 9, in <module>
      c1 = Color(255, 128, 0)
  TypeError: Color() takes no arguments
  ```

  This upgrades #732's own plan comment from a derived conclusion (observed
  only on 3.13.9) to directly observed on the 3.14.x line the project
  actually pins. `Color` has no real `__init__`, so instantiating it raises
  immediately. Any fixture that instantiates or calls a
  `dataclass_transform`-decorated class's generated methods therefore
  diverges from the CPython oracle by construction, for as long as pycc
  keeps its no-op-synonym divergence: this is not an unauthored-fixture gap
  (a future fixture could close that) or an unassessed row (a future
  assessment could resolve that) — it is structurally unflippable under
  `tests/conformance.rs`'s byte-for-byte comparison contract.

  This matters beyond one row because PEP 681 is one of
  [D-153](./D-153-correct-v0-3-s-conformance-target-before-any-v0.md)'s own
  19 itemized v0.3-implied-surface rows — the same itemized set PEP 487
  belongs to. [#585](https://github.com/rotnov/pycc/issues/585) already
  documents PEP 487 as unreachable under the same byte-for-byte contract
  (`__init_subclass__`/`__set_name__` are recognition-only; any fixture with
  observable hook output diverges from the oracle, and an inert hook body
  would prove nothing). D-153 anticipated some itemized rows failing and
  named an explicit 8-PEP fallback pool for exactly that
  case (PEPs 3102, 3104, 3132, 448, 604, 589, 586, 572) — but nothing
  recorded that *two* of the 19 itemized rows are now known-unreachable,
  which halves the itemization's remaining safety margin without anyone
  having said so until this decision.
- Decision: record, as project-wide and hard-to-reverse-if-missed policy,
  that PEP 681's matrix row cannot supply v0.3's 39th distinct PEP under
  pycc's current `dataclass_transform`-as-`@dataclass` design, exactly like
  PEP 487 cannot. If [#585](https://github.com/rotnov/pycc/issues/585)
  exercises its own first completion criterion's option to defer PEP 487's
  implementation, the 39th distinct PEP for v0.3 must come from D-153's own
  named fallback pool (3102, 3104, 3132, 448, 604, 589, 586, 572) — sourcing
  one PEP from a row outside the original 19-row itemization — or from a
  further, explicitly justified downward revision of the target under
  D-153's own amendment rule ("only lowering an already-corrected target
  again needs a new decision"). It must **not** come from PEP 681, and it
  must not be allowed to stall silently: this decision is the explicit
  record that deferring PEP 487 is not free with respect to v0.3's Accept
  clause once PEP 681 is also confirmed blocked.

  A cross-reference comment was added to
  [#585](https://github.com/rotnov/pycc/issues/585) naming this decision and
  the coupling. A new tracking issue records PEP 681's row as structurally
  blocked (not merely unassessed) and states the corrected fixture path and
  registration status, so the row carries a named owner for the honesty of
  its documentation, distinct from — and explicitly not attempting — the
  separate, out-of-scope decision that would actually make it flippable
  (removing the `dataclass_transform`-as-`dataclass` divergence).
- Alternatives:
  - **Flip PEP 681's row anyway, authoring a fixture that avoids
    instantiating the decorated class.** Rejected: the manifest's D-177
    acceptance rule requires every evidence-backed row to declare what its
    fixture actually proves, and a fixture that never exercises the
    generated methods proves nothing about `dataclass_transform` beyond "the
    decorator parses" — the same overclaim #248 already exists to prevent
    for other rows, applied here pre-emptively instead of after the fact.
  - **Remove pycc's `dataclass_transform`-as-`dataclass` divergence now, so
    the fixture's real behavior matches CPython.** Rejected as out of this
    decision's scope: it is a language-semantics reversal with its own
    design tradeoffs (would `dataclass_transform` become fully inert, or
    would pycc gain a real static-type-checker-only marker distinction it
    does not have today?), not a documentation or conformance-tooling fix.
    Left to the new tracking issue as a possible, but not mandated, future
    direction.
  - **Do nothing and leave the stale #248 citation in place.** Rejected: #248
    never discusses PEP 681, so the citation was actively misleading about
    where the row's status is tracked, and leaving it silently risks a
    future reader (or #585's own implementer) not realizing the two blocked
    rows compound into an Accept-clause risk.
- Consequences: v0.3's 39-distinct-PEP target now has two of its 19 itemized
  rows confirmed permanently unreachable (PEP 487, PEP 681) rather than one,
  which is now explicit rather than something a future session would have to
  rediscover independently. Whoever resolves #585 must consult this decision
  before deferring PEP 487, since doing so now has a documented Accept-clause
  consequence rather than being a free, isolated choice. If a future decision
  ever removes the `dataclass_transform`-as-`dataclass` divergence, this
  decision's blocking claim for PEP 681 becomes obsolete and should be
  superseded (not silently deleted), per this repository's convention of
  never rewriting an accepted decision in place.
