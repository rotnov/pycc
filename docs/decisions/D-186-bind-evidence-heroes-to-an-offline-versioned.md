---
id: D-186
title: "Bind evidence heroes to an offline versioned manifest with reviewed immutable attestations"
status: accepted
---

## D-186: Bind evidence heroes to an offline versioned manifest with reviewed immutable attestations

- Status: accepted
- Context: The public site had no common record for the source, command,
  output, test, revision, platform, and limitations shown by an evidence hero.
  The landing page's quick-start binding closed one concrete source/output
  drift path (#197), but every later Language, Diagnostics, Performance,
  Architecture, Status, Comparison, or Provenance hero could still invent a
  separate terminal-card convention. Fetching GitHub or another provider from
  pull-request CI would add mutable network state to an untrusted execution
  boundary, while accepting any syntactically valid commit or run identifier
  would still permit decorative provenance.
- Decision: `site/evidence-heroes.json` schema version `1.0.0` is the one
  ordered, allowlisted evidence-hero inventory. It carries exactly eight page
  and evidence kinds and the closed state vocabulary `all-Tier-1`, `partial`,
  `experimental`, `unavailable`, and `superseded`. An accepted hero records
  canonical fixture/test/snapshot paths and LF-normalized SHA-256 identities,
  exact commands and output, one full repository commit, an accepted workflow
  run, relevant Python/Rust/LLVM/profile/flag data, every Tier-1 runner/target
  and exact job link, limitations, immutable source/run links, and the public
  projection files. `scripts/check-site.sh` validates that record entirely
  offline: local artifact bytes and declared digests must equal the same blobs
  resolved from the exact local Git commit; the landing commit/run/platform
  tuple is a reviewed code allowlist; stable links are derived from those exact
  identifiers; and visible HTML, central Markdown, `llms.txt`, WebPage JSON-LD,
  and Open Graph/X description metadata must project the same evidence
  ID/kind/state. A page without an accepted unique hero retains every required
  field but sets the evidence-bearing fields to `null`, state to `unavailable`,
  and links only to its canonical owner. Existing explanatory pages may remain
  live in that state; `unavailable` describes their unique hero artifact, not
  the absence of the page and never a zero measurement.
- Alternatives: keep extending the landing-only #197 parser for each new page
  (rejected because eight incompatible mini-contracts would recreate the
  original drift); generate all public pages from JSON (rejected for version 1
  because it would replace the existing hand-authored, accessibility-reviewed
  static pages and enlarge this issue into a site generator); accept
  well-shaped but otherwise arbitrary commit/run/platform fields (rejected
  because decorative provenance remains possible); query GitHub during the
  required pull-request gate (rejected because provider availability and
  mutable responses do not belong in untrusted hermetic CI); mark planned
  heroes `partial` while carrying a convenient example (rejected because that
  is invented output, not partial evidence).
- Consequences: accepting or superseding a hero is an explicit reviewed change
  to the manifest, artifact bytes, immutable attestation allowlist, all named
  projections, documentation, and mutation tests. Cross-fixture combinations,
  missing files/tests/snapshots/records, commit or platform drift, moving
  `main` links, unknown fields/kinds/states, decorative values on unavailable
  records, and stronger HTML/Markdown/LLM/structured/social states all fail the
  canonical site gate. Mutation coverage removes every required field, replaces
  every evidence kind, deletes each accepted artifact, and independently
  overstates each projection family. The gate stores no credentials, cookies,
  account identity, sessions, or private traces and performs no live provider
  fetch. Child issues #565, #566, and #567 populate their records only after
  their own real artifacts and accepted attestations exist.
