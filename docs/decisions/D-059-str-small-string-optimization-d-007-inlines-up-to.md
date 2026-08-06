---
id: D-059
title: "`str` small-string optimization (D-007) inlines up to 22 UTF-8 bytes directly in the runtime string header (no heap allocation); longer strings heap-allocate with a refcount, no interning, no rope/cow structure -- the simplest representation matching D-007's own stated `≤ 22 bytes inline` threshold"
status: accepted
---

# D-059

Index-only: no long-form entry recorded yet.

`str` small-string optimization (D-007) inlines up to 22 UTF-8 bytes directly in the runtime string header (no heap allocation); longer strings heap-allocate with a refcount, no interning, no rope/cow structure -- the simplest representation matching D-007's own stated `≤ 22 bytes inline` threshold
