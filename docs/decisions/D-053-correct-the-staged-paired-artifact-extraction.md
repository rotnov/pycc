---
id: D-053
title: "Correct the staged paired-artifact extraction layout before activation"
status: accepted
---

## D-053: Correct the staged paired-artifact extraction layout before activation

- Status: accepted (corrects D-051's inert prospective workflow; D-048 remains the live transport until a separate activation pull request)
- Context: independent pre-activation review inspected the source of pinned `actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093` (v4.3.0) and found that its `artifact-ids` selector takes the multi-artifact extraction path even when the input contains exactly one ID. Without `merge-multiple: true`, each download adds its artifact name under the requested destination, while the reviewed fail-closed step requires `previous/estimates.json` and `current/estimates.json` directly. Activating the original D-051 fixture would therefore fail every run before comparison. The trusted `pull_request_target` policy still knows only the already merged D-051 shape and digest, so correcting the fixture and activating it in one pull request would also bypass the staged base-owned review boundary.
- Decision: keep `.github/workflows/ci.yml` byte-identical to the active D-048 steady-state fixture and retain both D-048 and D-051 authorizations during this corrective staging change. Add `merge-multiple: true` to each single-ID download in the inert `tests/fixtures/d51-paired-ci.yml`, require that exact input in the structural model, cover its removal from either download with fail-closed tests, and replace only the prospective D-051 SHA-256 with `4b1d11afba108745a2bc375e3447d92ecde843376c3bea95ab32f76b3fc53249`. After this staging change merges, a new branch from that exact default-branch state may activate the corrected fixture byte-for-byte and retire D-048.
- Alternatives: adjust the consumer to accept artifact-name subdirectories (rejected because the trusted upload IDs, not replaceable display names, are the intended input identities); use `name` so the action treats each invocation as a single-artifact download (rejected because deletion and same-name replacement would no longer fail closed); include staging and activation as separate commits in one pull request (rejected because the trusted workflow executes the default-branch base checker, not an earlier head commit from the same pull request).
- Consequences: this staging merge changes no live benchmark behavior and cannot repair the existing D-048 cross-host false failure by itself. It makes the exact extraction layout part of the reviewed D-051 contract and lets the subsequent fresh-base activation pass the base-owned policy without weakening artifact identity, the 2% threshold, comparator isolation, `ci-gate`, or the exact 100% line/region coverage invariant.

