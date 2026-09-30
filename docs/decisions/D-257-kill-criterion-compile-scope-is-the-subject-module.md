---
id: D-257
title: "The kill criterion's \"compiles unchanged\" covers the subject module, with sibling imports bound as foreign"
status: accepted
---

## D-257: The kill criterion's "compiles unchanged" covers the subject module, with sibling imports bound as foreign
- Status: accepted (resolves how [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 6's kill criterion scopes compilation, first applied to the #1207 workload; widens [D-252](./D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md) rule 1 to the subject module)
- Amendment (2026-09-30): #1366 delivers the D-222 opt-in this decision leaves to it as the `pycc build --ext` flag `--foreign-relative-imports`. It applies to the entry module's top-level relative from-imports only, and resolves them at import time against the package the artifact is imported under (`docs/CLI_SPEC.md`).
- Context: The #1207 workload is `lark` 1.3.1. Its subject is `ParserState.feed_token` in
  `lark/parsers/lalr_parser_state.py`, annotated under D-252. The annotated module's SHA-256 is
  `4335a1995da91fa264b3f16ebbb0c882863d0aba5d7d219205752fbf8a8680d9`. Every measurement up to
  2026-09-30 compiled the subject module in its package tree. That made pycc compile the module's whole
  import closure natively, because a relative import is a project import
  ([D-222](./D-222-project-modules-link-at-the-hir-level-into-one.md)). The build stops in
  `lark/utils.py`, and at `main` `05bc7805` it still reports 13 errors there. Four of the 13 are not
  compiler gaps:
  - two `T0001`s on unannotated parameters of a closure module, which D-252 does not reach;
  - two `T0002`s on `Any` used inside a dependency, which pycc refuses by design.

  Read as "the whole closure compiles natively", then, #1207's row (c) can never reach zero, however
  many compiler gaps close. The protocol's own text scopes the claim to the module:
  - the **Subject** bullet in `docs/TESTING.md` digests "the subject module's bytes";
  - #1207's row (c) is "pycc cannot compile the subject's module unchanged";
  - row (a) frames the sibling-import gap as compiling "a module that imports a sibling project module".

  The Cython arm (pure-Python mode) cythonizes that one module, while its sibling modules keep running
  in the interpreter. Row (a)'s "the workload's own module or modules" is the one phrase that could be
  read more widely. This decision settles that reading.
- Decision:
  1. **Scope.** For D-244 rule 6's kill criterion, the "compiles unchanged" claim covers the
     **subject module only**, that is, the module whose bytes `subject_sha256` digests. pycc's `ext` arm
     compiles that module. Its own imports of sibling workload modules (for `lark`, `from ..lexer
     import Token, LexerThread`, `from ..common import ParserCallbacks`, `from .lalr_analysis import
     Shift, ParseTableBase, StateT` and `from lark.exceptions import UnexpectedToken`) may resolve as
     foreign imports: CPython's own objects from the installed package, bound as an `object` the way
     #1278 binds any foreign from-import. This matches the Cython arm exactly. Each arm compiles the one
     module, and the interpreter provides its neighbours. The mechanism is #1366 (with #1138 for the
     dotted absolute form). Until #1366 lands and records its opt-in, D-222 is unchanged: an in-tree
     native build still links project modules natively.
  2. **Why this reading, and not favourability.** The whole-closure reading is not rejected because it
     is hard. It is rejected because it cannot be reached: pycc refuses `T0001` outside D-252's scope
     and `T0002` inside a dependency by design, not as gaps. So under that reading row (c) would record
     a miss that no compiler work could remove, which is the attribution error D-247 was written to
     prevent. The subject-module reading is no easier by construction. The by-design residue found in
     it so far stays on the blocker list, and the list grows if the unprobed regions of the
     module show more; that would not change this decision, because it is a statement about the
     subject rather than about its dependencies:
     - the subject's `-> Any` return (#1285);
     - `__eq__`'s unannotated `other`, whose only faithful annotation is the unspellable `object`
       (#1367).
  3. **What does not change.** The workload, the subject, the timed region, the input and its
     generator, the three arms, the outcome rows (a) to (e), rule 6's threshold, deadline and
     publication rule, and D-247's predicate are all unchanged. The compile-unchanged count is rule 6's
     corpus report metric ("a report, not the contract", `docs/ROADMAP.md`), and this decision does not
     touch it.
  4. **Annotation additions follow the compiled unit.** D-252 rule 1 admitted additions for "the kill
     criterion's hot function only". The unit that must compile is now the subject module, so D-252's
     annotation-only additions (logic unchanged, nothing replaced) may be made anywhere in the subject
     module, under D-252's own diff, digest and labelling rules. They are never made outside it.
  5. **Labelling.** A result recorded under this decision says which reading applied. The label is
     **"subject-module scope"**, carried alongside D-252's "annotated", wherever the result is
     reported.
  6. **The expectation is stated, not assumed.** The ≥ 5× expectation is doubtful anyway. With the
     sibling modules foreign, the loop body works almost entirely on objects pycc does not own: the
     parse table, the stacks, the callbacks, `Shift` and `UnexpectedToken`. So a compiled
     `feed_token` mostly calls back into CPython. `docs/TESTING.md` already records this. This decision
     is a scoping rule, not a prediction that the bet is won.
- Alternatives:
  - *Keep the whole-closure reading.* Rejected under rule 2: `T0001`/`T0002` inside `lark/utils.py`
    make it unreachable, so a miss under it would be about the workload's dependencies and never about
    the compiler.
  - *Also admit annotation additions to closure modules.* Rejected. It would remove the two `T0001`s
    but not the two `T0002`s. It would also stretch "logic unchanged, annotations added" to modules no
    arm is being timed on.
  - *Select another workload.* Rejected. #1207's stop rule forbids it.
  - *Leave the relative-import mechanism unresolved and record the scope only.* Partly adopted. The
    scope is settled here, and the mechanism, with its D-222 opt-in, is #1366's to design and record.
- Consequences: the utils.py chains (#1283, #1284, and the other rows of the `lark/utils.py` table in
  `docs/TESTING.md`) leave the subject's critical path. They stay open as ordinary breadth work. Row
  (c)'s blocker list becomes the subject module's own frontier. `docs/TESTING.md` measures it. It was
  probed piecewise, so it is a lower bound. What the 2026-10-22 record rests on is that list. Results
  can never be reported as whole-closure compiles. Rule 1 is a standing reading of D-244 rule 6,
  not a #1207 exception: a later kill-criterion workload is scoped the same way, and it publishes
  which sibling imports its subject module binds as foreign before any timed run.
