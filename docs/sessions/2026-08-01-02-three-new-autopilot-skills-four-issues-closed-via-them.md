# 2026-08-01 — Three new autopilot skills; four issues closed via them

**Authoritative checkpoint:** `main`'s tip is `e026fc6` (merge of
[PR #254](https://github.com/rotnov/pycc/pull/254)). This session added three
new project-local alpha skills — `issue-to-plan`, `issue-implement`,
`issue-select` — recorded in `docs/AGENT_TOOLING.md`'s "Project-local alpha
skills" section, plus a language rule in `AGENTS.md` (English for every
durable artifact, the user's own language for conversation). This entry's
own branch (`claude/new-skill-development-86a548`) is what carries that
work into `main`; see its pull request for the exact commit range.

**What each skill does:** `issue-to-plan` turns one GitHub issue into a
verified implementation plan, re-establishing the default branch and open
pull requests, treating the issue's own text as dated evidence to
re-verify rather than trust, and running an adversarial review loop before
publishing the plan as an issue comment. `issue-implement` takes one issue
end to end — staleness triage, plan acquisition, implementation under
D-021's preflight, the pinned D-068 deep-review loop, PR creation,
D-078-checkpointed CI/thread monitoring, and merge — with an enumerated,
issue-scoped public-write authorization so the run needs no per-step
confirmation. `issue-select` chooses the next issue for that pipeline:
full open-list inventory, a staleness screen that closes every provably
stale issue it finds, a blocker screen (dependency, roadmap/delivery-plan
mismatch, open-PR collision, maintainer-only authority), P1>P2>P3-then-
smaller-wins scoring, and an adversarial advisor round that answers "does
this need the maintainer?" so that question is never escalated to the
user.

**Four issues closed via this pipeline this session**, each merged
individually with its own PR, plan comment, and pinned-review history —
not summarized further here since each PR's own description is the
authoritative record:
[#238](https://github.com/rotnov/pycc/pull/238) (`pycc version --verbose`
contract, closing #38),
[#252](https://github.com/rotnov/pycc/pull/252) (module-value-binding
call-shadowing, D-110, closing #133 — 7 review rounds, 2 real blockers
found and fixed),
[#253](https://github.com/rotnov/pycc/pull/253) (`pycc init` overwrite
refusal, closing #237 — `issue-select`'s first live pick),
[#254](https://github.com/rotnov/pycc/pull/254) (missing-linker-driver
diagnostic, closing #250 — `issue-select`'s second pick).

**Lessons folded back into the skills as they were learned** (each skill's
own git history carries the full reasoning): `issue-to-plan` learned to
judge documentation-currency by a document's own granularity convention,
not by grepping for literal mentions, and to publish decision-log-entry
numbers derived from an open pull request as indicative rather than fixed.
`issue-implement` learned to verify review findings by reproducing the
predicted failure rather than re-deriving it, to treat a fix to a review
finding as *more* suspect than the original diff (the #133 run's two real
blockers were both introduced by fixes to earlier findings), and that a
shell pipeline can silently destroy a gate's real exit code
(`cmd | tail; echo $?` reports the pager's exit, not the gate's — this
nearly shipped a failing coverage run as green once). `issue-select`
learned to enumerate the complete same-priority peer set for its
adversarial advisor round rather than a curated shortlist, and to state
collision claims per layer (code vs. docs) rather than as an unqualified
"zero collision."

**Known gap, honestly recorded:** none of the three skills has bound
executable eval runners yet (`scripts/run_alpha_skill_evals.py` declares
no case for any of them), matching `pycc`/`pycc-feedback`'s own promotion
gate — binding those evals is a prerequisite before any of the three can
leave the project-local alpha set.

**Next session:** the `issue-select` loop is designed to keep running
(pick → plan → implement → merge → re-baseline) until the user stops it or
the pool has no survivors; resuming it needs no special setup beyond
invoking `/issue-select` again from a refreshed `main`.

