# 2026-09-11 (01) — #293: delete the stale D-112 shadow workflow

Status: delivered by the pull request that carries this file (squash-merged
into `main`); base `origin/main` `0bb17b28`.

## What changed

`.github/workflows/frontend-perf-shadow.yml` is deleted. It was PR #269's
temporary D-112 shadow-measurement workflow, `workflow_dispatch`-only, whose
header promised deletion "in the activation PR once evidence gathering,
Task 4, is done". The five evidence runs happened on 2026-08-01 and are
cited by run URL in D-112's Update; the activation (PR #278) landed the same
day without the deletion, so the file sat unused on `main` for six weeks.

`docs/ROADMAP.md`'s Quality gates row now describes those runs as runs of
the since-deleted temporary workflow, cited by URL in D-112. D-112 itself is
an accepted decision and is untouched. `docs/AGENT_RETROSPECTIVE.md` gains
the process lesson (a temporary artefact's own deletion promise is a plan
item to walk at activation).

## Why deletion instead of the pin #293 asked for

#293 asked the base-owned audit (`scripts/check_ci_permissions.rb`) to
require exactly `workflow_dispatch` on this file and to reject any other
trigger, with a negative mutation test. Re-verified on `0bb17b28`: the
checker constrains trigger *shape* only and deliberately accepts a
read-only `pull_request` workflow (`test_accepts_read_only_pull_request_job`),
so the issue's mutation still passed. An independent advisor round (D-127)
settled the fork: the mutation escalates no privilege (same read-only
token, no secrets) and only wastes runner minutes; a pin would add gate
surface for a file the tree had promised to remove, and the issue's own
expected item 4 ("keep the protection removable together with the temporary
workflow") names deletion as the terminal state. Deleting the file
satisfies that state directly and is the smaller sound change.

Verified before deleting: no script, workflow, or fixture references the
file (`git grep frontend-perf-shadow` outside `docs/decisions`,
`docs/sessions`, `docs/superpowers` hits only the file itself and the
ROADMAP row); `tests/fixtures/policy-successor-manifest.json` does not list
it; it is not `ci.yml`, so no D-103 digest cycle applies. The other
manual-only workflow, `hook-install-check.yml`, is intentionally permanent
and untouched.

## Gates at the delivered head

`check_roadmap_evidence.rb`, `test_check_roadmap_evidence.rb`,
`check_ci_permissions.rb` ("passed for 9 file(s)"),
`test_check_ci_permissions.rb`, `validate_agent_policies.py`,
`validate_agent_assets.py` all exit 0; `cargo doc --workspace --no-deps`
exit 0 at the D-021 preflight. No Rust source changes.

## Follow-ups

None filed. A future re-measurement (#296, #414) can restore the workflow
from the commit before this change; neither issue names it.

## Where to resume

`git log --oneline -1 origin/main`; open issues in milestone v0.4; PR #996
(part of #695) was the only open pull request when this was written.
