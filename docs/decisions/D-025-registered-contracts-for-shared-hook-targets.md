---
id: D-025
title: "Registered contracts for shared hook targets"
status: accepted
---

## D-025: Registered contracts for shared hook targets

- Status: accepted
- Context: D-023 permits a tracked fail-silent wrapper, but merely finding a hook target in Git does not prove that it tolerates an absent machine-local dependency. A tracked script or symlink can still invoke a missing `.ievo/hooks/` file and fail in every clean clone.
- Decision: this entry supersedes D-023 only for admitting shared hook targets. Every tracked filesystem target referenced by shared hook configuration must appear in `FAIL_SILENT_WRAPPER_CONTRACTS`, paired with a tracked `scripts/test_*.py` contract that is discovered by the required agent-policy test run. The contract must exercise the target from a clean-clone simulation with local generated hooks absent and assert a silent successful no-op. The registry is empty until such a wrapper and its security review are delivered.
- Authority and scope: registration permits only the named target in shared hook configuration. It does not permit generated hooks, local settings, arbitrary inline code, shell control operators, absolute or home-relative paths, loader URLs, or untracked files.
- Privacy and failure behavior: wrapper contracts use synthetic paths and inputs and must not access user state, credentials, or the network. A missing target, missing contract test, undiscovered test path, or unregistered target fails CI.
- Rollback: remove the shared hook entry and registry record together. Weakening the registry or accepting a wrapper without its clean-clone contract requires a superseding decision and security review.

