# 2026-08-31 — #24 Part 2: stage the rustfmt CI-gate check (D-080/D-215)

## Overall status

One session on branch `claude/issue-24-rustfmt-gate-stage`, off `main` at
`7107fc20` (Part 1, #860, already merged), preparing to open PR-B of #24 ("P2:
Add a rustfmt CI gate and format the merged Rust sources"). CI watch and
merge are pending and are recorded in the task's closing report, not here,
per D-192.

## What was delivered

- **Design decision, recorded as [D-215](../decisions/D-215-stage-a-tolerant-rustfmt-ci-gate-check-ahead-of.md):**
  `.github/workflows/ci.yml` is a D-171-routed workflow, so the pre-D-171
  D-080/D-090 whole-file-digest staging pattern does not transfer:
  `ci-gate`'s `needs`/failure condition are asserted with `==` against
  `D171_CI_GATE_NEEDS`/`D171_CI_GATE_FAILURE_CONDITION`, which are computed
  from `D171_OPTIONAL_ROUTING` rather than pinned by hash, and adding
  `rustfmt` to that map in this PR would immediately require the live
  `ci-gate` to depend on a job that does not yet exist, breaking `main`
  until a later activate PR lands. Instead, `scripts/check_roadmap_evidence.rb`
  gains an independent, additive check that accepts a D-171-routed workflow
  either with no `rustfmt` job (today's `main`) or with one in exactly the
  frozen shape below, wired into `ci-gate` the same way the map-computed
  constants would produce.
- `.github/workflows/ci.yml` is **not touched** in this PR, by design.
- `scripts/check_roadmap_evidence.rb`: added `D215_RUSTFMT_JOB`,
  `D215_RUSTFMT_CI_GATE_NEEDS`, `D215_RUSTFMT_CI_GATE_FAILURE_CONDITION`
  (computed from the existing `D171_CI_GATE_NEEDS`/
  `D171_CI_GATE_FAILURE_CONDITION`, not hand-duplicated), made the `ci-gate`
  needs/failure-condition assertions inside `validate_d171_ci_routing`
  conditional on whether a `rustfmt` job is present, and added
  `validate_optional_rustfmt_gate` to check the job's own shape when
  present. `REVIEWED_PERF_CI_WORKFLOW_SHA256S` is **not** extended — that
  allowlist is unreachable for a routed `ci-gate` (`d171_routed_workflow?`
  bypasses it), so a digest entry there would be dead weight.
- Target job design (frozen in `D215_RUSTFMT_JOB` and rendered into the
  fixture below): `runs-on: ubuntu-latest`, `needs: classify-changes`,
  `if: needs.classify-changes.outputs.compiler == 'true'` (same routing as
  `build-test-coverage`), the pinned checkout
  (`actions/checkout@d23441a4… # v6`, `persist-credentials: false`), the
  house `Show pinned toolchain` / `rustup show` step, then
  `rustup component add rustfmt` and `cargo fmt --all -- --check` with no
  `continue-on-error`.
- `tests/fixtures/d215-rustfmt-gate-ci.yml`: the complete intended final
  `ci.yml` bytes (current live file plus the new `rustfmt` job and the
  updated `ci-gate` `needs:`/`if:`), so the later activate PR can apply it
  verbatim instead of re-deriving the shape. Confirmed it independently
  passes `validate_d171_ci_routing`.
- `scripts/test_check_roadmap_evidence.rb`: 6 new tests — the fixture is
  accepted; a synthetic workflow with the job activated (built from the
  existing `d171_workflow` helper) is accepted; a job present but missing
  from `ci-gate`'s `needs` is rejected; `ci-gate` referencing `rustfmt`
  without the job existing is rejected; a malformed job body is rejected;
  `continue-on-error` on the job is rejected. Full suite: 250 runs / 1260
  assertions / 0 failures (was 244/1245 before this change).
- Noted, and worked around, a pre-existing YAML quirk shared with
  `D171_GOVERNANCE_POLICY_STEPS`: a whitespace-preceded `#` inside an
  unquoted plain scalar starts a YAML comment, so a step name like
  `"...issue #24)"` parses (and displays in the Actions UI) truncated at
  `"...issue"` — `D215_RUSTFMT_JOB`'s step name matches that truncated form
  intentionally, documented inline at its definition.
- `docs/decisions/README.md` regenerated
  (`python3 scripts/generate_decisions_index.py … --check` passes).

## Checks run locally

- `ruby scripts/test_check_roadmap_evidence.rb` (250/250) and
  `ruby scripts/check_roadmap_evidence.rb` (passes against the live,
  unmodified `ci.yml`) — both via
  `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby -E UTF-8 …` (the local zsh
  default locale throws `invalid byte sequence in US-ASCII` otherwise).
- `ruby scripts/test_check_ci_permissions.rb` / `ruby scripts/check_ci_permissions.rb`:
  unaffected (39/39, 0 failures) — this PR touches no `.github/workflows/*`
  bytes, confirming no staging was needed there.
- `cargo fmt --all -- --check`: exit 0 (unrelated pre-existing rustfmt
  warnings about escaped-newline string literals in
  `tests/slice1_codegen_depth.rs`, not failures).
- `git status --short` before staging: this PR touches only
  `scripts/check_roadmap_evidence.rb`, `scripts/test_check_roadmap_evidence.rb`,
  `docs/decisions/`, and one new fixture — no Rust source file changed.
  `cargo clippy`/`cargo llvm-cov` were **not** re-run locally on that basis
  (see Judgment call below); CI's own `build-test-coverage`/`native-build-test`
  jobs are the authoritative gate this PR is blocked on before merge.

## Known follow-ups

- **PR-C (activate)**: fold `"rustfmt" => ["compiler", "classify-changes"]`
  into `D171_OPTIONAL_ROUTING`, delete `validate_optional_rustfmt_gate` and
  its `D215_*` constants as dead code (D-014 does not cover Ruby, but an
  unreachable branch there is a review defect once the map is updated), and
  apply `tests/fixtures/d215-rustfmt-gate-ci.yml`'s bytes to `ci.yml` — all
  three in the same commit, since after the flip the tolerant path can never
  again be exercised. Re-run `cargo fmt --all` immediately before activating
  rather than assume `main` is still clean (nothing enforces it yet).
- Issue #24 stays open until PR-C lands.

## Judgment call recorded

The D-068 pinned reviewer's `ievo:deep-review` skill entrypoint could not be
invoked in this agent context: the `Skill` tool refused it with
`disable-model-invocation`, reserving it for explicit human/user invocation
(`/ievo:deep-review`) rather than agent dispatch, and no separate
agent-dispatch tool for the `deep-reviewer` agent was available in this
session's toolset. `python3 scripts/check_claude_reviewer_binding.py`
confirmed the structurally-verified install is present (`ievo@ievo-skills
0.80.19 OK`), so the binding itself is sound — the constraint is specific to
this session's ability to invoke it, not a missing/invalid reviewer. Per
`docs/AGENT_TOOLING.md`'s own instruction ("if a client cannot bind dispatch
to a structurally verified agent, report the local review as unavailable
instead of silently weakening the gate"), this is recorded here and in the
PR body rather than silently skipped; the change is small and mechanical
(new Ruby constants/validator plus a generated fixture, no runtime Rust
behavior), and CI's own required checks (`audit`, `governance`,
`build-test-coverage`, native/cross-compile matrices) still gate the merge.

Local coverage/clippy were not re-run directly against this diff (see
"Checks run locally" above) because it contains no Rust source changes at
all; re-running the full macOS-sandboxed `cargo llvm-cov` invocation for a
Ruby/YAML/docs-only diff would spend real wall-clock time confirming a
result CI's own `build-test-coverage` job already re-derives authoritatively
before merge is possible.
