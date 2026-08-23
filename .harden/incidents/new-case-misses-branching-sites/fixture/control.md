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

Write the plan as an ordered list of edits, each naming the file and the section it lands in.
