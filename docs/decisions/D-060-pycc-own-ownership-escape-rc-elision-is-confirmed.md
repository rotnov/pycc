---
id: D-060
title: "`pycc_own` (ownership/escape/RC-elision) is confirmed out of scope for PR-5, per DELIVERY_PLAN.md's v0.1 crate scope; every heap-allocated `str` this PR creates is unconditionally refcounted and freed on refcount reaching zero, with no cycle collector (D-004) since no v0.1 construct can form a reference cycle without classes/containers"
status: accepted
---

# D-060

Index-only: no long-form entry recorded yet.

`pycc_own` (ownership/escape/RC-elision) is confirmed out of scope for PR-5, per DELIVERY_PLAN.md's v0.1 crate scope; every heap-allocated `str` this PR creates is unconditionally refcounted and freed on refcount reaching zero, with no cycle collector (D-004) since no v0.1 construct can form a reference cycle without classes/containers
