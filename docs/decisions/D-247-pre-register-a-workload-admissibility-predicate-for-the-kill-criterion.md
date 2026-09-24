---
id: D-247
title: "Pre-register a workload-admissibility predicate for the D-244 rule 6 kill criterion"
status: accepted
---

## D-247: Pre-register a workload-admissibility predicate for the D-244 rule 6 kill criterion
- Status: accepted
- Amendment (2026-09-24, [#1207](https://github.com/rotnov/pycc/issues/1207)): this entry's admissibility predicate is unchanged. [D-252](./D-252-admit-annotation-only-additions-to-a-kill-criterion-subject.md) admits annotation-only additions to the subject of a workload this predicate admitted, which removes #1207's row (b) outcome for `lark` and leaves row (c) governing; a result under D-252 is labelled "annotated".
- Context: [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 6
  states `product-sprint-1`'s kill criterion as a ratio measured on "the reference
  codebase's hot loop", and `docs/TESTING.md`'s "Hosted `ext` benchmark protocol
  (product-sprint-1)" Subject bullet fixes the methodology for measuring it. Both
  presume that a given workload *has* such a referent, and neither states a predicate
  that decides whether it does. Two CPU profiles of the reference workload, taken from a
  tree pinned to a 23-commit February 2026 window and analyzed on 2026-09-22 against
  `20a845ff`, show that this workload does not: rank 1 by self time is a third-party
  C-extension routine in both profiles (22.06% and 19.88% of summed self time, five
  calls each -- neither Python nor a loop), roughly 26% more is a numeric library's own
  pure-Python dispatch layer rather than the workload's code, and the single
  workload-owned entry in either top ten (12.73% / 13.00% self, 51.2% / 52.6%
  cumulative) exists at `HEAD` only in a rewritten form with no statement loops, its
  surviving successor being module-private and therefore non-exported under
  [D-038](./D-038-a-leading-underscore-marks-a-top-level-function.md) and
  [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 1.
  Separately, none of the six array-taking, loop-bearing methods the compile-unchanged
  census identified builds unedited, because the enclosing module's import graph is
  refused before any candidate is reached -- but that is a compiler gap that will close,
  whereas the profile finding holds with the import boundary entirely open. An AOT
  compiler has nothing to offer a hot path that is already native, so leaving rule 6 as
  written would let a property of the workload be recorded as a verdict on pycc.
- Decision: rule 6's threshold and publication rule are unchanged; this entry narrows
  rule 6 by adding the predicate it presumed. A workload is an admissible subject source
  only when a CPU profile taken on it under the protocol's stated conditions shows a
  single function that is (i) the workload's **own** source rather than a dependency's or
  the interpreter's, (ii) Python rather than a C extension, (iii) loop-bearing by
  statement loops -- a comprehension alone does not qualify -- and (iv) responsible for at
  least **20%** of summed self time. The share is fixed here, before any candidate
  workload is profiled, and is derived from arithmetic rather than from an observation:
  at one fifth exactly five disjoint own-code functions can clear the bar at once, and
  every lower bar admits more simultaneous claimants, so "the hot loop" stops
  designating anything well before the share reaches zero. Whether the chosen
  function is *exported* is deliberately **not** part of this predicate -- that is a
  property of the subject, already governed by
  [D-244](./D-244-add-a-hosted-cpython-extension-module-artifact-mode.md) rule 1 and by
  [D-038](./D-038-a-leading-underscore-marks-a-top-level-function.md), and refusing a
  whole workload over how its author packaged one helper would reject on a fixable
  detail while calling it a workload property. A workload that fails this predicate
  yields no subject and is refused, and that refusal is a finding about the workload,
  never a measurement of pycc. The current reference workload fails clause (ii) at rank
  1 and clause (iii) at `HEAD` for its own hottest entry, so it is refused; the 20%
  share is not load-bearing for that refusal, which is why fixing the share in this
  entry does not set a bar against an already-seen result.
- Alternatives:
  - *Amend the Subject bullet to designate the hot loop from a freshly taken,
    pre-registered profile.* Rejected: it addresses the profiles' age, which is not the
    defect. A fresh profile's rank 1 would still be a C-extension routine, because the
    absence of a referent is a property of the workload's shape.
  - *Declare rule 6's criterion answered negatively for this workload and let the kill
    date decide on that basis.* Rejected: "pycc did not reach the threshold" and "this
    workload's hot path was already native" are different claims, and only the second was
    measured. Recording the first would attribute a workload property to the compiler.
  - *Select a replacement workload directly.* Rejected as stated, admitted under this
    entry's predicate. Choosing a workload after seeing this result is the same
    inadmissibility the protocol's Input and Arms bullets already rule out: a workload
    picked for looking favourable decides the bet instead of measuring it. A predicate
    fixed in advance is what makes a replacement admissible.
  - *Drop the kill criterion.* Rejected: the criterion is what makes the bet falsifiable,
    and its deadline is the mechanism that stops the sprint from running indefinitely.
- Consequences: the kill criterion becomes measurable again, at the cost of one extra
  pre-registered step -- a profile and a predicate check -- before any arm is timed. Its
  2026-10-22 deadline is judged against a run on an admissible workload, so a workload
  that could never produce a subject no longer counts as evidence either way. The current
  reference workload is refused, which is irreversible in the sense that matters: this
  entry records why, so a later session cannot re-adopt it without superseding this
  decision. `docs/TESTING.md`'s protocol section and
  [#1039](https://github.com/rotnov/pycc/issues/1039) carry the predicate's operational
  form; `subject_sha256` in `scripts/bench_hosted_ext_precommit.json` stays `null` until
  a run on an admissible workload registers it, as that run's own first action. Because
  a replacement workload brings its own input, that record's `input_sha256`, `seed`,
  `generator_path` and `compile_unchanged_*` fields are re-registered rather than carried
  over, in their own pre-registration commit ahead of any run — a separate event from
  `subject_sha256`'s, which keeps the Subject bullet's rule unchanged. Finding a replacement workload is new
  work this entry does not do and does not schedule.
