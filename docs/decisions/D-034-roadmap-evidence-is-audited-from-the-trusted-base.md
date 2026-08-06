---
id: D-034
title: "Roadmap evidence is audited from the trusted base revision"
status: accepted
---

## D-034: Roadmap evidence is audited from the trusted base revision

- Status: accepted
- Context: a checked roadmap item is a release claim under D-013, but a checker and coverage job executed from the pull-request head can be replaced together by the same untrusted pull request. Keeping the required GitHub check names while turning both implementations into successful no-ops would make an in-branch evidence gate self-attesting.
- Decision: regular pull-request CI still runs the head checker for fast feedback, but it is not the authority. The required read-only `Workflow policy` job checks out the base commit, downloads the head revision's workflow files and `docs/ROADMAP.md` as non-executable data, and runs the base revision's roadmap tests and checker against that data. The trusted checker binds each accepted claim to its exact heading path, claim text, and workflow proof; it ignores any replacement checker present in the head revision. It validates the hard 100% coverage workflow unconditionally, even when the corresponding roadmap claim is unchecked or absent, so removing all markers cannot weaken the merge invariant. Changes to the trust-anchor workflow retain D-020's staged SHA-256 allowlist process.
- Alternatives: trust the pull-request copy of the checker (rejected because the same change controls both policy and proof); rely only on required check names (rejected because names say nothing about the commands that ran); require human review of each roadmap marker (rejected as non-deterministic and incompatible with the solo-maintainer workflow).
- Consequences: a pull request cannot authorize a new evidence type or weaken an existing proof in one self-contained change. The trusted checker and prospective trust-anchor digest land first; only a later pull request may activate the audited head inputs or mark the claim complete. Every new evidence type needs a mutation proving the base checker rejects a forged head proof. Once an approved trust-anchor replacement becomes active, its reviewed fixture remains a byte-identical audit snapshot and the superseded digest is retired.

