---
id: D-022
title: "Autonomous public iEvo bug reporting"
status: accepted
---

## D-022: Autonomous public iEvo bug reporting

- Status: accepted (privacy and failure behavior clause below superseded by [D-087](#d-087-relax-ievo-bug-report-privacy-scrubbing-to-credentials-only) — see that decision for the current scrubbing rule; every other clause here remains in force unchanged)
- Context: reproducible defects in the iEvo control plane can affect every agent task until upstream fixes ship. Requiring a new permission round-trip after the user has adopted the project policy delays correction and encourages local-only workarounds that other users cannot discover.
- Decision: after reproducing an iEvo malfunction, contradiction, broken hook, or invalid command and searching open and closed upstream issues, agents may create or update a public `ievo-ai/skills` issue without additional per-report permission. Duplicates, expected behavior, unverified suspicion, and ordinary pycc failures are not reportable.
- Authority and scope: the standing authority covers only the minimum useful iEvo report and evidence. It does not authorize publishing unrelated pycc material, reporting to other projects, or changing upstream code/labels/issue state beyond the report itself.
- Privacy and failure behavior (superseded — see D-087): this clause originally required reports to omit credentials, personal data, private/proprietary content, raw conversations, and identifying local paths even when the pycc repository is public. D-087 narrows this to credentials/secrets/API keys/tokens only; everything else may be included as-is when it makes a report more actionable. The failure-handling half of this clause is unchanged by D-087: if authentication/network submission fails, preserve the pending sanitized report locally and surface the failure; do not repeatedly spam the endpoint.
- Rollback: a public report cannot be made private retroactively. A mistaken report is corrected and closed with an explicit explanation. Revoking standing authority requires a superseding decision and removal of the matching AGENTS.md section.

