# Evidence Model

Use this model to distinguish discovery material from conclusions.

## Evidence types

| Type | What it can establish | Common limitation |
| --- | --- | --- |
| Source and tests at a fixed revision | Current implemented behavior | Intent may be undocumented |
| Merged PR and commit | A change was accepted | It may not be released or may be reverted |
| Release notes and tags | A fix was shipped in a version | Notes may omit edge conditions |
| Maintainer explanation | Intended cause or design | Can predate later findings |
| Minimal reproducer | Symptom under stated conditions | May not prove the root cause |
| Independent reports | Breadth and recurrence | Reports can share the same underlying source |
| User speculation | Search vocabulary and hypotheses | Not evidence of causality |

Primary evidence is project-owned source, tests, commits, release artifacts, and
maintainer records. Blog posts and summaries may orient the search but should
not carry a material conclusion when primary evidence is available.

## Claim states

Classify each important claim:

- **Reported:** a participant observed or asserted it.
- **Reproduced:** steps or a test demonstrate the symptom.
- **Diagnosed:** evidence identifies a causal mechanism.
- **Fixed in source:** a merged change addresses the mechanism.
- **Released:** a tagged or documented release contains the fix.
- **Current:** the present source or documentation still reflects the claim.
- **Inferred:** your conclusion connects evidence that does not state it
  directly.
- **Contradicted:** credible evidence disputes the claim.

Never collapse these states into the word "fixed".

## Confidence

### High

Use only when the causal chain is confirmed by strong primary evidence, such
as a reproducer plus merged test and fix, and applicability to the user's
version is established.

### Medium

Use when multiple pieces of evidence support the mechanism but one link is
missing, such as release status, exact version boundary, or a direct
reproducer.

### Low

Use for a plausible mechanism supported mainly by reports, analogies, or an
unverified patch. Present it as a hypothesis and recommend a discriminating
test.

Do not average confidence across unrelated claims. One finding may contain a
high-confidence symptom and a low-confidence cause.

## Independence and deduplication

Two URLs are not independent evidence when:

- one is a duplicate or cross-post of the other;
- both quote the same original report;
- an issue, PR, and commit describe one change;
- multiple downstream reports share the same upstream regression;
- generated summaries restate the same primary source.

Cluster them into one evidence chain. Look for a genuinely separate
reproducer, implementation, or maintainer analysis before claiming
independent confirmation.

## Applicability test

For every finding compare:

| Dimension | Questions |
| --- | --- |
| Version | Is the affected code present? Was the fix released or reverted? |
| Platform | Do OS, architecture, runtime, or filesystem assumptions match? |
| Configuration | Are the triggering flags, features, or defaults enabled? |
| Architecture | Does the same invariant exist in the user's design? |
| Scale | Does the failure require concurrency, data volume, or uptime absent here? |
| Trust boundary | Can the same actor or input reach the vulnerable path? |

Label the result:

- **Directly applicable:** same mechanism and preconditions.
- **Conditionally applicable:** relevant only if named conditions hold.
- **Analogical:** different implementation but a transferable invariant.
- **Not applicable:** preconditions or affected code are absent.
- **Unknown:** local or upstream evidence is insufficient.

## Contradictions

When credible sources disagree:

1. show the disagreement;
2. compare dates, versions, and authority;
3. check whether the sources describe different configurations;
4. prefer current source over older prose for implemented behavior;
5. lower confidence if the conflict remains unresolved.

Do not silently choose the conclusion that best fits the initial hypothesis.
