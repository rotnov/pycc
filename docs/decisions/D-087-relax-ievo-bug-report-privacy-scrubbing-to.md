---
id: D-087
title: "Relax iEvo bug-report privacy scrubbing to credentials-only"
status: accepted
---

## D-087: Relax iEvo bug-report privacy scrubbing to credentials-only

- Status: accepted
- Context: D-022 authorized autonomous public bug reports to `ievo-ai/skills` but scoped their privacy scrubbing broadly -- "omit credentials, personal data, private/proprietary content, raw conversations, and identifying local paths." In practice this makes reports about pycc-specific iEvo failures (e.g. a hook interacting badly with this repo's own D-023/D-025 machine-local-hook policy) hard to reproduce upstream without the exact repository context that scrubbing would strip. This repository is explicitly experimental with a single maintainer (`denis@27tech.co`/`rotnov`) and has no other contributors' data to protect; the tradeoff D-022 made for a general-audience privacy default does not apply the same way here.
- Decision: for iEvo-specific bug reports only, relax the scrubbing requirement to credentials/secrets/API keys/tokens alone. Personal information, private repository data, proprietary source or documentation, raw conversation text, and identifying local paths may all be included as-is when doing so makes the report more actionable. This does not change D-022's other constraints (duplicate search first, no reporting of expected behavior/unverified suspicion, standing authority scoped to the report itself).
- Alternatives: keep D-022's broad scrubbing (rejected -- the user explicitly requested this relaxation, judging the extra reproducibility worth the tradeoff for their own single-maintainer, experimental project). Relax scrubbing for all upstream/external reporting repository-wide (rejected as broader than requested -- this decision is scoped to iEvo bug reports specifically, not e.g. any future third-party integration).
- Consequences: `AGENTS.md`'s D-022 section is updated to state the narrowed credentials-only scrubbing rule for iEvo reports, linking here. Future iEvo bug reports may include repository-identifying detail that D-022's original wording would have required redacting; credentials/secrets/API keys/tokens remain redacted unconditionally.

