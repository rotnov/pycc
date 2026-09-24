---
id: D-252
title: "Admit annotation-only additions to a kill-criterion subject (owner directive)"
status: accepted
---

## D-252: Admit annotation-only additions to a kill-criterion subject (owner directive)
- Status: accepted (owner directive of 2026-09-24 on [#1207](https://github.com/rotnov/pycc/issues/1207); narrows [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 6's kill-criterion clause and #1207's outcome row (b))
- Context: [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 6 states the
  kill criterion as "the reference hot function compiles unchanged as an extension module and runs ≥ 5×
  faster than CPython". `docs/TESTING.md`'s protocol **Subject** bullet makes this operational: "an arm
  that edits the source to make it compile has failed rather than scored". The replacement workload
  that [D-247](./D-247-pre-register-a-workload-admissibility-predicate-for-the-kill-criterion.md)'s
  predicate admitted under #1207 is `lark` 1.3.1. Its subject is `ParserState.feed_token(self, token:
  Token, is_end=False) -> Any`, and `is_end` has no annotation. #1207's pre-registered row (b) therefore
  recorded the criterion as not met, without a timed run. On 2026-09-24 the repository owner directed
  that this is a chicken-and-egg outcome and that the annotations should simply be fixed. The reasoning:
  pycc compiles annotated Python, and a public workload that is annotated except for one defaulted flag
  does not tell us whether pycc can meet the bet. Under
  [D-127](./D-127-autonomous-agent-operation-model.md) an owner intervention takes precedence over the
  pre-registered row.
- Decision:
  1. **What is admitted.** A kill-criterion subject may be compiled with **annotation-only additions**.
     This means a type annotation *added* to a parameter, a return or a local variable that had none.
     Nothing else may change: no statement, no expression, no default value, no import, and no existing
     annotation may be replaced or removed. The rule is "logic unchanged, annotations added", and
     rule 6's "compiles unchanged" is read that way for the kill criterion's hot function only.
  2. **Publication.** The addition is published as an exact diff against the pinned source, together
     with the SHA-256 of the original module and of the annotated module. The Subject bullet's
     `subject_sha256` is then the digest of the annotated bytes. All three arms still time
     byte-identical source: that is, the annotated bytes.
  3. **Labelling.** A result obtained under this rule is labelled **"annotated"**, never "unchanged",
     wherever it is reported. That includes the protocol report, the roadmap and any issue record.
  4. **What is not narrowed.** The **compile-unchanged count** is rule 6's metric and
     `product-sprint-1`'s second acceptance item. It still counts only functions whose source is
     byte-identical to the original, with no annotation additions. Rule 6's threshold, deadline and
     publication rule are unchanged, and D-247's admissibility predicate is unchanged.
  5. **Effect on #1207.** For `lark`, row (b) no longer governs, because the subject can be fully
     annotated by one addition, `is_end: bool = False`. The outcome that applies is row (c), under which
     compiler gaps are worked until 2026-10-22 and whatever is still open at the deadline is recorded as
     the miss. `docs/TESTING.md`'s #1207 status subsection carries the diff, the compile result and the
     current gap list. The row-(b) record stays there as history.
- Alternatives:
  - *Keep row (b) as the final outcome.* Rejected by the owner's directive. It would record a
    workload's missing annotation on one defaulted `bool` flag as a verdict on the compiler, which is
    the same attribution error D-247 was written to prevent.
  - *Permit any edit that makes the subject compile.* Rejected. A logic edit is a rewrite, and a
    rewritten subject can be tuned toward the bet, which is exactly the selection-after-the-result
    D-247 and the protocol's Input bullet rule out. An added annotation cannot change how CPython runs
    the code, because CPython does not enforce annotations at run time.
  - *Also permit replacing an existing annotation, for example `-> Any` with a concrete type.*
    Rejected. The directive was to fix *missing* annotations. A replacement annotation can assert a
    type the code does not honor, and choosing it is a judgment an evaluator could bias.
  - *Select another workload.* Rejected. #1207's stop rule forbids it, and every row of its outcome
    table says a failure after admission never moves to the next candidate.
- Consequences: the kill criterion is again decided by a measurement or by named compiler gaps, rather
  than by one missing annotation. The first compile under this rule, on pycc `20c2c76d` on 2026-09-24,
  shows that the annotation was not the binding constraint. The diagnostics with and without the
  annotation are identical apart from the directory name. The first module in the subject's import
  closure to fail is `lark/utils.py`, with 18 errors whose first is `C0001` ("import of module
  `itertools` is not supported yet"). So row (c)'s gap list is what the 2026-10-22 decision now rests
  on. Results under this rule can never be reported as "compiled unchanged". A later workload may use
  the same rule, provided it publishes the diff before any timed run.
