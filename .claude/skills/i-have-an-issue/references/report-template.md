# Report Template

Adapt the size to the question, but preserve the evidence fields.

## Outcome

State the engineering conclusion in two to five sentences:

- the most important confirmed or plausible failure mechanisms;
- whether they apply to the current project;
- the first prevention or diagnosis actions to take.

Label inference explicitly.

## Research scope

- **Mode:** reactive or preventive
- **Project context:** component, versions, platforms, constraints
- **Repositories:** exact upstreams and analogues, with selection rationale
- **Surfaces:** issues, discussions, PRs, commits, source, tests, releases
- **Coverage date:** date of research
- **Important exclusions:** what was intentionally not searched

## Findings

| Finding | Trigger | Evidence state | Applicability | Recommended guard | Confidence |
| --- | --- | --- | --- | --- | --- |
| Short mechanism name | Preconditions | Diagnosed / fixed / released / inferred | Direct / conditional / analogical / none / unknown | Test, assertion, metric, or design change | High / medium / low |

Keep the table compact. Put evidence links next to the supported text.

## Detailed finding: `<mechanism>`

- **Observed symptom:** What maintainers or users saw.
- **Failure mechanism:** The confirmed cause, or a clearly labeled hypothesis.
- **Triggering conditions:** Versions, platforms, configuration, scale, or
  timing.
- **Evidence chain:** Report → reproducer or diagnosis → PR or commit → test →
  release, with stable links.
- **Contradictions:** Reverts, dissent, incompatible reports, or missing links.
- **Applicability:** Which local code or invariant matches, and which does not.
- **Action:** Concrete mitigation or design choice.
- **Guardrail:** Regression test, assertion, telemetry, rollout check, or
  operational runbook item.
- **Confidence:** Rating plus the reason it is not higher.

Repeat only for findings that affect the decision.

## Rejected leads

List plausible but excluded results and one-line reasons. This is especially
important when the research produced few findings.

## Gaps and next experiment

State what public evidence cannot answer. Recommend the smallest reproducer,
benchmark, fault-injection test, source inspection, or version bisect that
would discriminate between the remaining hypotheses.

## Sources

List primary sources grouped by repository. Avoid repeating links already clear
in the findings unless a consolidated list improves navigation.

## Negative-result variant

When no credible precedent is found, do not return an empty report. Include:

- exact scope and repositories covered;
- representative query families;
- weak leads rejected and why;
- whether absence of evidence is surprising;
- residual risks inferred from the architecture;
- the next local experiment.
