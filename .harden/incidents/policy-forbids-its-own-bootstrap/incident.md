---
topic: policy-forbids-its-own-bootstrap
date: 2026-08-23
gap: absence
termination: none
artifact: none
fixture: none
verify: n/a
verdict: pending
---

Symptom: a change introduced a quantitative creation gate and, in the same
document, mandated that a new kind of container entity be created — without
checking whether the gate admits the mandated creation. As drafted the policy
was unimplementable: the first agent to follow it would have had to violate the
rule it was implementing in order to implement it. Caught in the first review
round and fixed by an explicit bootstrap exemption.

Root cause, identifier-stripped: a document that both constrains an action and
mandates that action is checked against itself only if someone thinks to read
its two halves together. Nothing mechanical detects that a numeric ceiling
makes a mandated entity uncreatable — it is not a reference-integrity or
vocabulary property, so the co-occurrence and link checkers this project
already runs are structurally blind to it.

Singleton, so no artefact per the batch pass's own threshold rule: this entry
is the counter seed. A second occurrence of "new gate versus the creation the
same document mandates" clears the threshold and takes the ladder. Deliberately
not folded into `new-case-misses-branching-sites`, whose members are missing
branches for a new case rather than a rule contradicting its own mandate, nor
into the `summary-tier-contradicts-its-own-body` family, whose members are
drift between restatements of one fact.

Batch: `.harden/findings/issue-734.jsonl`.
