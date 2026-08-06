---
id: D-057
title: "PR-5's MIR stays a typed structural mirror of HIR (not real SSA); LLVM codegen uses one `alloca` per local/parameter + `load`/`store`, relying on no optimization pass (correct and simplest for a `--debug`-only v0.1 profile per D-034/DELIVERY_PLAN.md)"
status: accepted
---

# D-057

Index-only: no long-form entry recorded yet.

PR-5's MIR stays a typed structural mirror of HIR (not real SSA); LLVM codegen uses one `alloca` per local/parameter + `load`/`store`, relying on no optimization pass (correct and simplest for a `--debug`-only v0.1 profile per D-034/DELIVERY_PLAN.md)
