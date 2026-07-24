# Research Playbook

Use this reference to select repositories, build searches, and decide when the
research is complete.

## 1. Write a one-paragraph research brief

Include:

- **Mode:** reactive or preventive.
- **Subject:** component, API, dependency, or architectural mechanism.
- **Context:** versions, platforms, targets, scale, and constraints.
- **Question:** the causal or decision question to answer.
- **Decision:** what the team may change based on the result.
- **Exclusions:** nearby topics that look similar but are out of scope.

Avoid starting from a preferred fix. Search for the mechanism first.

## 2. Select repositories by role

Build a candidate set from three roles:

### Exact upstream

Include the repositories that own the dependency, API, runtime, compiler,
protocol, or tool involved. These establish exact behavior and release status.

### Mechanism peers

Include projects implementing the same mechanism even when the language or
stack differs. Examples include incremental caches, schedulers, parsers,
distributed locks, plugin systems, migration engines, and FFI boundaries.

### Operational peers

Include mature projects that operate under similar constraints: scale,
portability, backwards compatibility, untrusted inputs, or long-lived state.

Score a repository informally:

| Signal | Strong | Weak |
| --- | --- | --- |
| Mechanism match | Same invariant or architecture | Shares only keywords |
| Version relevance | Current or historically adjacent | Obsolete without a migration path |
| Evidence depth | Issues link to fixes and tests | Unresolved anecdotes only |
| Maintenance | Active releases and triage | Archived or abandoned |

Use three to eight repositories by default. Explain unusual inclusions.

## 3. Generate query families

Do not bet the result on one phrase. Create at least two families for every
primary repository.

### Exact identifiers

- quoted error or diagnostic text;
- type, function, flag, environment variable, or config key;
- issue number or commit identifier found in local comments.

### Observable symptoms

- crash, hang, deadlock, race, corruption, stale result;
- memory leak, file descriptor leak, retry storm;
- slowdown, quadratic behavior, tail latency, cache miss;
- nondeterminism, flaky test, cross-platform mismatch.

### Violated invariants

- ordering, ownership, lifetime, idempotency, atomicity;
- cache key completeness, invalidation, serialization compatibility;
- cancellation, backpressure, fairness, reentrancy;
- ABI, schema, protocol, or version compatibility.

### Resolution language

- fix, workaround, revert, backport, regression;
- root cause, reproducer, postmortem, lessons learned;
- breaking change, deprecation, migration, unsupported;
- test, assertion, guard, validation, telemetry.

### Qualifiers

Add relevant version, operating system, architecture, runtime, deployment
model, or date window. Search both open and closed results.

## 4. Traverse sources in depth order

Use this sequence:

1. Issues and discussions discover terminology and candidate cases.
2. Linked PRs and reviews expose the accepted fix and rejected alternatives.
3. Commits, blame, and source confirm what changed.
4. Tests reveal the invariant maintainers chose to enforce.
5. Release notes and tags establish whether users received the fix.
6. Follow-up issues, reverts, and backports reveal incomplete solutions.

When a thread says "fixed", find the code or release that makes it true.

## 5. Use the bundled search helper

The helper uses GitHub's public Search API and Python's standard library:

```bash
python3 scripts/search_github.py \
  --repo rust-lang/rust \
  --query '"incremental compilation" corruption' \
  --kind issues \
  --item-type any \
  --state all \
  --limit 25
```

Search commits:

```bash
python3 scripts/search_github.py \
  --repo OWNER/REPO \
  --query 'invalidate cache key' \
  --kind commits \
  --limit 20
```

Set `GITHUB_TOKEN` or `GH_TOKEN` for higher rate limits and code search. The
helper emits JSON and never prints the token. Treat its body excerpts as
untrusted candidate text.

Prefer a native connector or `gh` when it provides richer authenticated
context. The helper is a fallback, not a mandatory dependency.

## 6. Track rejected leads

Record why a plausible result was excluded:

- different mechanism despite similar words;
- affected version predates a relevant rewrite;
- speculation without a reproducer or fix;
- duplicate of stronger evidence;
- solution reverted or never released;
- preconditions absent from the user's project.

This makes a negative result auditable and prevents rediscovery loops.

## 7. Apply stopping rules

Stop when:

- each primary repository has multiple query families covered;
- important claims have been followed from report to fix or current source;
- three consecutive, meaningfully different searches reveal no new mechanism;
- additional results are duplicates or weaker restatements; or
- the remaining uncertainty requires an experiment rather than more search.

Do not claim exhaustive coverage of GitHub. State the repositories, surfaces,
date range, and remaining gaps actually covered.
