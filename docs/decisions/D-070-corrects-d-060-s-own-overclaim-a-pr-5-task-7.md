---
id: D-070
title: "Corrects D-060's own overclaim (a PR-5 Task 7 review finding, not a change of decision): \"every heap-allocated `str` ... freed on refcount reaching zero\" describes the intended object model, not Task 7's actual codegen scope -- refcount calls are only ever inserted at codegen-time binding sites (see D-061's Consequences for the full, corrected accepted-leak scope: unbound temporaries, loop-body-first bindings, and a never-reassigned function-scoped local all leak, memory-safely, until `pycc_own` (v0.5) lands)"
status: accepted
---

# D-070

Index-only: no long-form entry recorded yet.

Corrects D-060's own overclaim (a PR-5 Task 7 review finding, not a change of decision): "every heap-allocated `str` ... freed on refcount reaching zero" describes the intended object model, not Task 7's actual codegen scope -- refcount calls are only ever inserted at codegen-time binding sites (see D-061's Consequences for the full, corrected accepted-leak scope: unbound temporaries, loop-body-first bindings, and a never-reassigned function-scoped local all leak, memory-safely, until `pycc_own` (v0.5) lands)
