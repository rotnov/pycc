---
id: D-052
title: "Share immutable function registries across type-check environments"
status: accepted
---

## D-052: Share immutable function registries across type-check environments

- Status: accepted
- Context: D-041 correctly requires each function-body check to clone the completed module environment so local parameter and assignment bindings cannot leak across functions. The function registry in that environment is immutable once `check_with_signatures` begins its body-check pass, but a plain `HashMap` clone still copied every function name, parameter vector, and return type for every function. The required frontend benchmark measures this path, and same-host component timing identified type checking as a material part of the parse/lower/check budget.
- Decision: store `Environment`'s function registry in an `Arc<HashMap<…>>` and use `Arc::make_mut` only while registering functions or when the standalone `check_function` API must add its own recursive signature. Module-driven function checks share the already complete registry, borrow the registered parameter slice while binding locals, and continue to clone the ordinary bindings map. This supersedes D-041 only for how the immutable function registry and its resolved signature are read; global and local binding isolation is unchanged.
- Alternatives: keep deep-cloning the registry (rejected as repeated allocation with no semantic benefit); use `Rc` (rejected because it would unnecessarily remove `Send`/`Sync` compatibility from the public `Environment`); borrow the registry through a lifetime-parameterized environment (deferred because it would expand the public type and API surface for the same immutable-sharing result).
- Consequences: function checks avoid repeated function-registry and registered-parameter allocation while preserving lookup behavior, standalone recursion, and clone isolation through copy-on-write mutation. Against the same saved local Criterion baseline, the complete parse/lower/check fixture moved from a `7443.3 ns` mean to `6979.6 ns`; Criterion reported a `-6.23%` mean change with a `-7.40%` to `-4.76%` interval. Tests cover mutation after cloning, direct `check_function`, sibling calls, global visibility, and the hard 100% line/region gate.

