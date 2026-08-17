---
id: D-170
title: "Extend the status-page freshness check with a third signal: feature-landing paragraph set changes (#522)"
status: accepted
---

## D-170: Extend the status-page freshness check with a third signal: feature-landing paragraph set changes (#522)

- Status: accepted
- Context: D-156's two-signal design (milestone-line change, evidence-checklist
  state change) has a detection gap. A new feature-landing paragraph added under
  an existing `## vX.Y` heading in `docs/ROADMAP.md` — the
  `**[#NNN](https://github.com/rotnov/pycc/issues/NNN) — <feature>:**` convention
  used for every landed v0.3 class-model item — changes neither the
  `**Current milestone: ...**` line nor a `<!-- roadmap-evidence: ... -->`
  checklist entry, so the original check reported `pass` while the status and
  landing pages drifted past six landed v0.3 items (#432 inheritance, #377
  `@property`, #378 dataclasses, #379 enums, #380 protocols, #381 structural
  pattern matching). #522 identified this gap: the status page's "Class model
  core" evidence card and "Still ahead for v0.3" prose, and `docs/WEBSITE.md`'s
  boundary description, all still listed those six items as planned when they
  had already landed.
- Decision (extend D-156 to three signals, do not supersede it): add a third
  signal to `scripts/check_status_page_freshness.rb`. A new
  `FEATURE_PARAGRAPH_MARKER` regex
  (`/\*\*\[#(\d+)\]\(https:\/\/github\.com\/rotnov\/pycc\/issues\/\d+\)\s*—/`)
  matches the opening of every feature-landing paragraph line and captures the
  issue number. `feature_paragraph_ids(text)` returns the sorted array of
  captured issue numbers, and `roadmap_signal?` now returns true if ANY of the
  three signals fire: the milestone line, the evidence-checklist states, or the
  feature-paragraph id set. D-156's core decision (mechanism (a), the narrowly
  scoped CI check, not mechanism (b), full auto-generation) stands unchanged;
  only the signal count is extended. D-156 is not superseded.
- Decision (set membership, not text diff): the third signal compares the *set*
  of feature-paragraph issue numbers between base and head, not the paragraph
  text. An addition (a new `**[#NNN](...) — ...:**` paragraph) or a removal (an
  existing one deleted) both change the set and fire the signal. A text-only
  modification to an existing paragraph — same issue number, different
  description — leaves the set unchanged and does not fire. This mirrors how
  `evidence_checklist_states` compares hash equality rather than line text, and
  preserves the gate's signal-to-noise ratio: routine description edits to an
  already-landed feature do not require a watched-page touch, while the
  appearance or disappearance of a feature-landing paragraph — a real status
  change — does.
- Consequences: a pull request that adds or removes a feature-landing paragraph
  in `docs/ROADMAP.md` without touching either watched page now fails the
  freshness check with an actionable message mentioning the feature-landing
  paragraph signal, closing the exact silent-drift failure mode #522 reported.
  The check remains intentionally narrow — it verifies that *some* watched-page
  update accompanied the signal, not that the page content is accurate; content
  accuracy stays a review-time judgment. The `FEATURE_PARAGRAPH_MARKER` regex is
  specific to the established `**[#NNN](https://github.com/rotnov/pycc/issues/NNN) — `
  prefix with the em dash; if the roadmap's feature-paragraph format evolves
  (a different dash character, a different URL pattern), the regex must be
  updated in lockstep. `docs/ROADMAP.md` itself needed no content change from
  this decision. This entry supersedes nothing; it extends D-156.
- Alternatives: an unconditional "any `docs/ROADMAP.md` diff must touch a
  watched page" trigger (already rejected by D-156 — fires on routine
  prose-only edits with no status implication, training maintainers to ignore
  the gate); addition-only detection rather than set comparison (rejected — a
  removal is also a significant status change that should trigger a page update,
  and set comparison handles both uniformly); full text diff of feature
  paragraphs (rejected — fires on description-only edits to an existing
  paragraph, the same over-firing risk the set-membership design avoids).
