---
id: D-172
title: "Use a base-owned property audit without forced CI successor activation"
status: accepted
---

## D-172: Use a base-owned property audit without forced CI successor activation

- Status: accepted
- Context: PR #562 left `main` in a D-103 contradiction: unchanged CI failed the exact successor transition, while exact D-171 activation ran five stale self-test assertions. Two merges did not prevent the same maintainer from staging and later activating weak policy, but did make unrelated PRs unmergeable.
- Decision: keep required `audit` base-owned under `pull_request_target`, download candidate workflows only as data, and validate permissions, Action pins, event-default candidate checkout and credential bindings, trusted-event guards, D-171 routing, runner and execution-context bindings, unconditional native/Pages proofs, Tier-1 coverage, and `ci-gate` truth-table properties. General CI/checker files no longer require predecessor-staged whole-file bytes. D-125 remains external-state-only. The owner authorized one D-024 relaxation of `audit` for the #558 recovery PR; `ci-gate` remains required and protection is restored immediately.
- Consequences: ordinary CI changes use one PR; historical successor fixtures remain evidence but cannot force activation. Trust-anchor workflow changes remain separately protected. The recovery records its protection snapshot, one-merge/ten-minute window, exact merge SHA, restore readback, and independent post-restore verification.
- Update (2026-08-26): D-203 records the counterexample class to "ordinary CI changes use one PR" — a change that must alter both a frozen gate-defining checker constant and its mirrored ci.yml step still needs the two-PR coexist-then-retire lifecycle, because the base-owned audit validates the head's ci.yml against the pre-PR checker's constants.
