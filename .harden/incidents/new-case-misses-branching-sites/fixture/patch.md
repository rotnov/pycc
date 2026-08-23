# Project rules

## Planning a change

A plan is written before any code or document is edited. It states what to change and where,
grounded in the current tree rather than in the issue's own framing.

Read what governs the change, not what merely mentions it: the document that owns the affected
area, the accepted decisions the change touches, and the conventions the surrounding files
already follow.

Distinguish, explicitly and in the plan, between a **merge gate** (something fails without it)
and a **file convention** (the surrounding text does it, but nothing enforces it). Presenting a
convention as a gate makes the next reader do work that was never required; presenting a gate as
a convention makes them ship something broken.

The documentation-currency check has its own trap: whether a change needs a documentation update
is decided by the owning document's granularity convention, not by whether that document
literally mentions the changed surface. Grepping for existing mentions of the exact surface and
finding none proves nothing.

When the change adds an exceptional case to a rule an existing document already branches on,
the plan enumerates **every site that dispatches on the general rule**, not only the sites the
request names. Salience and decision-relevance are different sets: the request names where the
new case is interesting, while the sites that will get it wrong are wherever the document
already asks "which case is this?" and now has one answer too few. Derive that list by searching
for the rule's own decision points — the places that branch, not the places that mention — and
record it in the plan as the change's affected-site inventory, with a line per site saying
whether the new case needs a branch there or provably does not.

Write the plan as an ordered list of edits, each naming the file and the section it lands in.
