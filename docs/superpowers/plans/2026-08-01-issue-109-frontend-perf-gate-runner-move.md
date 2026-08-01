# Issue #109 remedy: move `frontend-perf-measure`/`frontend-perf-gate` off `macos-14` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `frontend-perf-gate`'s repeated false-positive regression failures (issue #109's multi-month history: D-048→D-051→D-053→D-056→D-062, each closed then reopened by fresh hosted-runner-variance evidence) by moving `frontend-perf-measure`/`frontend-perf-gate` from `runs-on: macos-14` to `runs-on: ubuntu-latest` — the runner class this project has already, independently, found to be its least-noisy (see Context below) — gathering real shadow-measurement CI evidence before committing, then activating for real.

**Architecture:** This project's CI trust-anchor (`scripts/check_roadmap_evidence.rb`) pins the exact byte content of `.github/workflows/ci.yml`'s `frontend-perf-measure`/`frontend-perf-gate` jobs via (a) a whole-file SHA256 allowlist and (b) a strict deep-equality structural check against hardcoded Ruby hash literals — changing `runs-on` on either job requires a brand-new pair of job constants, not an edit to an allowlist. Because `.github/workflows/workflow-policy.yml`'s `pull_request_target` audit always executes `main`'s own (not the PR's) copy of `check_roadmap_evidence.rb` against the PR's proposed `ci.yml`, a PR cannot introduce a new checker shape and the `ci.yml` that matches it in the same commit — the checker code must land on `main` first (**stage**), then a later PR can change the live workflow to match it (**activate**). This plan adds a third phase between them — **shadow-measure** — because this specific remedy is, by the issue's own most recent analysis, an unproven hypothesis about hosted-runner behavior, not a known-good fix: a temporary, non-required `workflow_dispatch` job runs the *actual* new-shape benchmark on `ubuntu-latest` for real, several times, so the decision to activate is made from CI evidence, matching this project's own established evidentiary bar (D-095/D-096/D-101's "before/after CI evidence, not local hunch").

**Tech Stack:** Ruby (`scripts/check_roadmap_evidence.rb`, `scripts/test_check_roadmap_evidence.rb`, Minitest), GitHub Actions YAML (`.github/workflows/ci.yml`, `.github/workflows/workflow-policy.yml`), `apt.llvm.org`'s `llvm.sh` (already-reviewed Linux LLVM-22 install pattern), Criterion (`benches/check_bench.rs`, untouched by this plan).

## Global Constraints

- **User authorization**: the user explicitly delegated this specific remedy choice ("сам решай, ты же в автопилоте") after being shown both candidates (runner-move vs. `getrusage` CPU-time measurement) and their relative costs; runner-move was chosen as the cheaper, less-invasive option (no new `[[bench]]` target, no new `libc` dev-dependency, so it does not self-trip `frontend-perf-measure`'s own `contract_paths`/`Cargo.toml`-tail hard-abort checks the way the `getrusage` approach would).
- **D-014**: 100% line/region coverage remains a hard merge gate on every ordinary PR touching Rust code; this plan touches no Rust source, only Ruby/YAML, so `cargo llvm-cov` is unaffected, but `ruby scripts/test_check_roadmap_evidence.rb` must stay green throughout and grow tests for every new constant/branch this plan adds.
- **D-024 / branch protection**: `main` accepts changes only through PRs; the two-phase split below is not optional (`workflow-policy.yml`'s audit always runs the *base* branch's checker against the candidate `ci.yml` — see D-100's own "Update" note, `docs/DECISIONS.md`).
- **D-025 / registered contracts for shared hook targets**: `scripts/check_roadmap_evidence.rb` is exactly the kind of trust-anchor content D-025 protects; every new constant this plan adds must have a corresponding tracked test in `scripts/test_check_roadmap_evidence.rb`, run in required CI, before the PR that introduces it merges.
- **No new elevated CI permissions**: the temporary shadow-measure job (Task 3) must use `permissions: {}` or `contents: read` only, run on `pull_request`/`workflow_dispatch` (not `pull_request_target`), and must not become a required check — it is a throwaway measurement tool, removed in Task 6.
- **D-number collision discipline**: the highest ADR heading in `docs/DECISIONS.md` on `origin/main` at plan-write time is `D-111`; **Task 1 must re-verify this against the live `origin/main` state immediately before writing**, since another concurrent branch (e.g. the still-open PR #236) may have already claimed D-112+ by execution time. This plan provisionally refers to the new ADR as **D-112**, correcting the number at write time if collided, per this project's own established D-090→D-091 / D-051→D-056→D-062 / D-110's-own-documented-gap convention (never silently renumber a *published* ADR — if collision is found after this plan's own D-112 has already been committed and referenced elsewhere in this same plan's execution, add the real number and leave a dated note, mirroring D-110's own text).
- **Context already verified (do not re-derive)**: the live `frontend-perf-measure` job is `.github/workflows/ci.yml:480-704`, live `frontend-perf-gate` is `.github/workflows/ci.yml:706-812`. Today's active structural constants are `D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB` (measure) and `REPLICATED_PERF_GATE_JOB` (gate), recognized via `validate_source_aware_perf_gate_lifecycle` (`scripts/check_roadmap_evidence.rb:1186-1236`), gated behind the sole active whole-file digest `D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256` in `REVIEWED_PERF_CI_WORKFLOW_SHA256S` (`scripts/check_roadmap_evidence.rb:186-188`). `frontend-perf-measure`'s current macOS LLVM install is exactly:
  ```yaml
        - name: Install LLVM 22 (D-015)
          run: brew install llvm@22
        - name: Export LLVM_SYS_221_PREFIX
          run: echo "LLVM_SYS_221_PREFIX=$(brew --prefix llvm@22)" >> "$GITHUB_ENV"
  ```
  The already-reviewed Linux equivalent, verbatim from `native-build-test`'s Linux legs (`.github/workflows/ci.yml:206-215`), to reuse unconditionally (drop the `if: runner.os == 'Linux'` guard — this job's `runs-on` is unconditionally `ubuntu-latest`):
  ```yaml
        - name: Install LLVM 22 (Linux, via apt.llvm.org)
          run: |
            wget https://apt.llvm.org/llvm.sh
            chmod +x llvm.sh
            sudo ./llvm.sh 22
            # llvm.sh's own packages don't pull in Polly's static lib; llvm-sys
            # links it explicitly, so it must be installed separately here.
            sudo apt-get install -y libpolly-22-dev
            echo "LLVM_SYS_221_PREFIX=/usr/lib/llvm-22" >> "$GITHUB_ENV"
  ```
  `build-test-coverage` (also on `macos-14`) is a **separate, unrelated** trust anchor (`coverage_gate_present?`, allowlist-based, hardcodes its own `"macos-14"` literal at `scripts/check_roadmap_evidence.rb:1084`) — **out of scope for this plan**, not touched.
  `validate_source_aware_perf_gate_lifecycle`'s comparison is **whole-hash deep equality** (`measure_job == D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB`, etc.) — there is no partial/allowlist path for these two jobs (that only exists for `build-test-coverage`), so a `runs-on` change requires an entirely new job constant plus a new `elsif` branch, not an edit to an existing one.
  `scripts/test_check_roadmap_evidence.rb` has 131 tests; each historical/staged whole-workflow digest has a matching literal fixture under `tests/fixtures/*ci.yml` referenced by a `Pathname` constant at the top of the file. The richest concrete precedent to copy is `test_d91_relax_frontend_perf_manifest_workflow_remains_an_audit_fixture` (staged-but-inactive assertion shape) and `test_d100_composed_workflow_is_active_and_reviewed` (full activation assertion shape).

## PR breakdown

1. **Stage PR** (Tasks 1-2): new ADR + new Ruby job/gate constants + new fixture + new digest constant (excluded from the active array) + tests. Does not touch live `ci.yml`. Merges to `main` first.
2. **Shadow-measure PR** (Task 3, can be the same PR as the Stage PR or a fast-follow — implementer's call, record the choice in the ledger): adds a temporary, non-required `workflow_dispatch` job that runs the real new-shape benchmark on `ubuntu-latest`.
3. **Evidence gathering** (Task 4): trigger the shadow job several times via `gh workflow run`, record results in the ADR as a dated Update note.
4. **Activation PR** (Task 5): flip live `ci.yml` to the new shape, flip `REVIEWED_PERF_CI_WORKFLOW_SHA256S`, retire the old digest to historical, remove the shadow job, update tests and docs.
5. **Close-out** (Task 6): verify CI green post-merge including `frontend-perf-gate` itself, close issue #109 with acceptance evidence (only if the evidence genuinely supports it — if shadow evidence contradicts the hypothesis, STOP and report to the user instead of activating; do not force this closed).

---

### Task 1: Record the ADR

**Files:**
- Modify: `docs/DECISIONS.md` (append new ADR after D-111)

**Interfaces:**
- Consumes: issue #109's full comment history (already read this session — cite the job-duration CV table finding: `ubuntu-latest` 3.1% CV vs. `macos-14`/`macos-15-intel` 11.2%-25.9%, and the correction that the runner-move is not a one-line change because of the 3-layer pinning described in Global Constraints).
- Produces: the ADR number (provisionally D-112, re-verify first) that Tasks 2-6 reference in commit messages and code comments.

- [ ] **Step 1: Re-verify the next available D-number**

  Run:
  ```bash
  git fetch origin main
  git show origin/main:docs/DECISIONS.md | grep -oE "^## D-[0-9]+" | sort -t- -k2 -n | tail -3
  ```
  If the highest is still D-111, use **D-112**. If a higher number already exists (e.g. PR #236 has since claimed D-112+), use the next free number and note the collision inline exactly as D-110's own text does ("this entry is numbered D-N because open PR #M has already published claims on D-X–D-Y").

- [ ] **Step 2: Write the ADR**

  Append to `docs/DECISIONS.md`, after the current last entry:

  ```markdown
  ## D-112: Move `frontend-perf-measure`/`frontend-perf-gate` from `macos-14` to `ubuntu-latest`

  - Status: proposed (see Consequences — activation is gated on Task 4's shadow-measurement evidence, not assumed here)
  - Context: issue #109 has been reopened four times since its first "resolved" comment (D-048→D-051→D-053→D-056→D-062), each time by fresh evidence that `frontend-perf-gate`'s paired-comparison measurement flips outcome on byte-identical compiler source across independent CI runs on `macos-14`. The most recent investigation (issue #109, 2026-08-01) pulled `native-build-test` job-duration coefficients of variation across every runner class this workflow uses: `ubuntu-latest` 3.1%, `windows-latest` 4.7% (excluding one known Defender-scan outlier class already documented at D-096), `ubuntu-24.04-arm` 15.4%, `macos-14` (via `build-test-coverage`, a different job, no Criterion involved) 11.2%, `macos-15-intel` 25.9%. This is prior art independent of anything Criterion-specific: D-095/D-096/D-101 already found and accepted that non-`ubuntu-latest` runners are noisier for this project's *other* timing gate (nbody), lowering that gate's floor per-target rather than assuming uniform runner behavior. `ubuntu-latest` has never needed a relaxed gate in this project's history. A `getrusage(RUSAGE_SELF)` CPU-time measurement backend was also prototyped and works, but requires a new `[[bench]]` target and a `libc` dev-dependency, both of which sit inside `frontend-perf-measure`'s own hard-abort `contract_paths`/`Cargo.toml`-tail checks — the PR introducing it would trip its own gate by construction. The runner-move needs neither.
  - Decision: stage new Ruby job constants `D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB`/`D112_UBUNTU_FRONTEND_PERF_GATE_JOB` in `scripts/check_roadmap_evidence.rb`, identical to the live `D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB`/`REPLICATED_PERF_GATE_JOB` except `"runs-on" => "ubuntu-latest"` and the LLVM-22 install step swapped for the already-reviewed `apt.llvm.org` pattern `native-build-test`'s own Linux legs already use (see Global Constraints for both step bodies verbatim). Gather real shadow-measurement CI evidence (Task 4) that this reduces false-positive variance before activating; do not activate on the job-duration proxy alone (job duration includes checkout/toolchain-install/compile time, not just the timed Criterion samples — a weaker signal than a direct measurement of the actual gate).
  - Alternatives: `getrusage` CPU-time measurement (rejected for now per Context above — self-trips the gate's own hard-abort checks; remains a documented fallback if the runner-move's own shadow evidence is unfavorable). Lower the 2% threshold instead (rejected — this project's own explicit, repeated position throughout #109's history is that the gate is a spec requirement, not a knob to relax to force a pass, exactly as D-096/D-101 already rejected the equivalent move for the nbody gate). Do nothing further, treat #109 as permanently open (rejected — it currently blocks PR-10 and, transitively, the entire remaining v0.2 milestone from merging).
  - Consequences: this ADR's Status will be updated to `accepted` (with a dated Update note, mirroring D-100's own convention) once Task 4's shadow evidence supports activation, or to a documented alternate path if it does not. `build-test-coverage` (still `macos-14`) is unaffected by this decision — it is a separate trust anchor with its own, unrelated pinning mechanism (`coverage_gate_present?`).
  ```

- [ ] **Step 3: Commit**

  ```bash
  git add docs/DECISIONS.md
  git commit -m "docs: record D-112, propose frontend-perf-gate runner move to ubuntu-latest"
  ```

---

### Task 2: Stage the new job/gate constants, fixture, digest, and tests (no live `ci.yml` change)

**Files:**
- Modify: `scripts/check_roadmap_evidence.rb`
- Create: `tests/fixtures/d112-ubuntu-frontend-perf-ci.yml`
- Modify: `scripts/test_check_roadmap_evidence.rb`

**Interfaces:**
- Consumes: `D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB`, `REPLICATED_PERF_GATE_JOB` (existing constants, `scripts/check_roadmap_evidence.rb:809-825`), `validate_source_aware_perf_gate_lifecycle` (`scripts/check_roadmap_evidence.rb:1186-1236`).
- Produces: `D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB`, `D112_UBUNTU_FRONTEND_PERF_GATE_JOB`, `D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256` — consumed by Task 5 (activation).

- [ ] **Step 1: Define the new measure-job constant**

  In `scripts/check_roadmap_evidence.rb`, immediately after `D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB`'s definition (currently ending at line 825), add:

  ```ruby
  D112_UBUNTU_FRONTEND_PERF_MEASURE_STEPS =
    Marshal.load(Marshal.dump(D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_STEPS)).tap do |steps|
      llvm_index = steps.index { |step| step["name"] == "Install LLVM 22 (D-015)" }
      raise "expected an existing macOS LLVM-install step to replace" unless llvm_index

      export_index = steps.index { |step| step["name"] == "Export LLVM_SYS_221_PREFIX" }
      raise "expected an existing LLVM_SYS_221_PREFIX export step to remove" unless export_index

      steps.delete_at(export_index)
      steps[llvm_index] = {
        "name" => "Install LLVM 22 (Linux, via apt.llvm.org)",
        "run" => <<~SHELL.strip
          wget https://apt.llvm.org/llvm.sh
          chmod +x llvm.sh
          sudo ./llvm.sh 22
          # llvm.sh's own packages don't pull in Polly's static lib; llvm-sys
          # links it explicitly, so it must be installed separately here.
          sudo apt-get install -y libpolly-22-dev
          echo "LLVM_SYS_221_PREFIX=/usr/lib/llvm-22" >> "$GITHUB_ENV"
        SHELL
      }
    end.freeze
  D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB = D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB.merge(
    "runs-on" => "ubuntu-latest",
    "steps" => D112_UBUNTU_FRONTEND_PERF_MEASURE_STEPS
  ).freeze
  D112_UBUNTU_FRONTEND_PERF_GATE_JOB = REPLICATED_PERF_GATE_JOB.merge(
    "runs-on" => "ubuntu-latest"
  ).freeze
  ```

  **Note for the implementer**: `D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_STEPS` is itself derived from `REPLICATED_PERF_MEASURE_STEPS` via `.tap` (see `scripts/check_roadmap_evidence.rb:815-822`) — confirm its exact step ordering (`grep -n '"name" =>' scripts/check_roadmap_evidence.rb` around that region) before writing the `.tap` block above, since a mismatched `llvm_index`/`export_index` pair silently produces a malformed steps array that will only surface as a fixture round-trip failure in Step 3 below, not a Ruby error. Verify the two step names ("Install LLVM 22 (D-015)", "Export LLVM_SYS_221_PREFIX") are exact string matches against the live constant before relying on `.index`.

- [ ] **Step 2: Add the new digest constant and stage it (excluded from the active array)**

  Immediately after `D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256`'s definition, add (placeholder value — Step 3 computes and fills in the real digest):

  ```ruby
  # Staged (2026-08-01, D-112): ubuntu-latest frontend-perf-measure/gate.
  # Not yet in REVIEWED_PERF_CI_WORKFLOW_SHA256S -- see D-112's own
  # activation task before this becomes the live-accepted digest.
  D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256 =
    "PLACEHOLDER_REPLACE_WITH_REAL_DIGEST_IN_STEP_3"
  ```

  Do **not** add this constant to `REVIEWED_PERF_CI_WORKFLOW_SHA256S` (`scripts/check_roadmap_evidence.rb:186-188`) — it must remain `[D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256]` until Task 5.

- [ ] **Step 3: Build the fixture file and compute its real digest**

  Create `tests/fixtures/d112-ubuntu-frontend-perf-ci.yml` as a full byte-for-byte copy of the *current* live `.github/workflows/ci.yml`, with only `frontend-perf-measure`'s and `frontend-perf-gate`'s bodies replaced to match the new constants exactly (same technique as every prior staged fixture — e.g. `tests/fixtures/d91-relax-frontend-perf-manifest-ci.yml` was built the same way relative to its own predecessor). Concretely: `runs-on: macos-14` → `runs-on: ubuntu-latest` on both jobs, and replace the two-step LLVM install (`Install LLVM 22 (D-015)` + `Export LLVM_SYS_221_PREFIX`) with the single new `Install LLVM 22 (Linux, via apt.llvm.org)` step body from Global Constraints, verbatim.

  Then compute the real digest and fill in the placeholder from Step 2:
  ```bash
  ruby -rdigest -e 'puts Digest::SHA256.file("tests/fixtures/d112-ubuntu-frontend-perf-ci.yml").hexdigest'
  ```

  **Verify the fixture round-trips structurally** before moving on — this is the step most likely to silently diverge from the Ruby hash constant (e.g. YAML block-scalar indentation differences that parse to the same string are fine; accidental extra/missing keys are not):
  ```bash
  ruby -e '
    require "./scripts/check_roadmap_evidence"
    text = File.read("tests/fixtures/d112-ubuntu-frontend-perf-ci.yml")
    stream = Psych.parse_stream(text, filename: "fixture")
    root = yaml_mapping(stream.children.first.root, "fixture")
    jobs = yaml_mapping(root["jobs"], "fixture jobs")
    measure = yaml_value(jobs["frontend-perf-measure"], "fixture measure")
    gate = yaml_value(jobs["frontend-perf-gate"], "fixture gate")
    raise "measure job mismatch" unless measure == D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB
    raise "gate job mismatch" unless gate == D112_UBUNTU_FRONTEND_PERF_GATE_JOB
    puts "fixture matches constants"
  '
  ```
  (`yaml_mapping`/`yaml_value` are module-level methods in `check_roadmap_evidence.rb` — requiring the file directly makes them available; adjust the `require` path/invocation style to match however the script's own test file already loads it, e.g. `scripts/test_check_roadmap_evidence.rb`'s own top-of-file requires.)

- [ ] **Step 4: Add the new structural-recognition branch**

  In `validate_source_aware_perf_gate_lifecycle` (`scripts/check_roadmap_evidence.rb:1186-1236`), extend the `elsif` chain:

  ```ruby
  expected_perf_job =
    if measure_job == D56_SOURCE_AWARE_PERF_MEASURE_JOB
      D56_SOURCE_AWARE_PERF_GATE_JOB
    elsif measure_job == REPLICATED_PERF_MEASURE_JOB
      REPLICATED_PERF_GATE_JOB
    elsif measure_job == D91_RELAX_FRONTEND_PERF_MANIFEST_MEASURE_JOB
      REPLICATED_PERF_GATE_JOB
    elsif measure_job == D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB
      D112_UBUNTU_FRONTEND_PERF_GATE_JOB
    end
  ```

- [ ] **Step 5: Write the staged (not-yet-active) tests**

  In `scripts/test_check_roadmap_evidence.rb`, add a `Pathname` constant alongside the existing fixture constants (near line 16-36):
  ```ruby
  D112_UBUNTU_FRONTEND_PERF_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d112-ubuntu-frontend-perf-ci.yml"
  ```

  Add tests modeled directly on `test_d91_relax_frontend_perf_manifest_workflow_remains_an_audit_fixture` (staged/inactive assertion) and the `validate_source_aware_perf_gate_lifecycle`-acceptance half of `test_d99_vcpkg_libxml2_cache_workflow_digest_matches_the_reviewed_fixture`:
  ```ruby
  def test_d112_ubuntu_frontend_perf_workflow_digest_matches_the_staged_fixture
    assert_equal(
      D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D112_UBUNTU_FRONTEND_PERF_WORKFLOW_FIXTURE).hexdigest
    )
  end

  def test_d112_ubuntu_frontend_perf_workflow_is_staged_not_active
    refute_includes REVIEWED_PERF_CI_WORKFLOW_SHA256S,
                    D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256
  end

  def test_d112_ubuntu_frontend_perf_workflow_structure_is_recognized
    workflow_text = D112_UBUNTU_FRONTEND_PERF_WORKFLOW_FIXTURE.read
    assert validate_source_aware_perf_gate_lifecycle(workflow_text, D112_UBUNTU_FRONTEND_PERF_WORKFLOW_FIXTURE.to_s)
  end
  ```

  Also add at least one **mutation/rejection** test proving the new constant's `runs-on` really is checked, following the `paired_perf_workflow`/mutate/assert-raises pattern (`scripts/test_check_roadmap_evidence.rb:119-156`, e.g. mirroring `test_rejects_paired_measurement_without_an_exact_candidate_checkout`):
  ```ruby
  def test_rejects_d112_measurement_job_with_a_different_runner
    mutated = Marshal.load(Marshal.dump(D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB))
    mutated["runs-on"] = "ubuntu-24.04" # plausible near-miss, not the pinned value
    workflow = { "jobs" => { "frontend-perf-measure" => mutated, "frontend-perf-gate" => D112_UBUNTU_FRONTEND_PERF_GATE_JOB } }
    workflow_text = { "jobs" => workflow["jobs"] }.to_yaml
    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow_text, "mutated.yml")
    end
    assert_includes error.message, "reviewed source-aware measurement job"
  end
  ```
  (Adjust the exact YAML-construction helper to match whichever of the file's existing helpers — e.g. `source_aware_perf_workflow` — already builds a minimal `{"jobs" => {...}}` document; do not hand-rebuild YAML serialization if a helper already exists for this shape.)

- [ ] **Step 6: Run the full Ruby test suite and fix anything the new constants break**

  ```bash
  LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/test_check_roadmap_evidence.rb
  LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb .
  ```
  (The `LANG`/`LC_ALL` prefix works around this shell's default `US-ASCII` locale crashing on the file's existing em-dashes — a pre-existing, unrelated quirk both prior implementers already worked around; do not "fix" the em-dashes themselves.) Both must exit 0 and print no new failures. `check_roadmap_evidence.rb .`'s own output must still say `Roadmap evidence policy passed.` — the live `ci.yml` is unchanged in this task, so this just confirms the new staged code didn't break anything active.

- [ ] **Step 7: Commit**

  ```bash
  git add scripts/check_roadmap_evidence.rb scripts/test_check_roadmap_evidence.rb tests/fixtures/d112-ubuntu-frontend-perf-ci.yml
  git commit -m "ci: stage D-112 ubuntu-latest frontend-perf-measure/gate job shape (no live change)"
  ```

---

### Task 3: Add a temporary shadow-measurement job

**Files:**
- Modify: `.github/workflows/ci.yml` (add a new, non-required job; do not touch `frontend-perf-measure`/`frontend-perf-gate` themselves)

**Interfaces:**
- Consumes: the LLVM-install pattern and benchmark-running steps already present in `frontend-perf-measure` (Global Constraints).
- Produces: real Criterion timing data for Task 4 to evaluate.

- [ ] **Step 1: Add `frontend-perf-shadow-ubuntu`, triggered by `workflow_dispatch` only**

  Add a new job to `.github/workflows/ci.yml`, not wired into `ci-gate`'s `needs`/`if` (so it can never block a PR), that runs the *current* candidate commit's frontend fixture benchmark on `ubuntu-latest`, twice (predecessor vs. itself is not the point here — the point is measuring **within-runner-class variance** across repeated same-source runs, matching the issue's own "repeated unchanged-code measurements" acceptance-criteria language):

  ```yaml
    # Temporary (D-112 shadow-measurement, remove in the activation PR): runs
    # the candidate frontend-perf fixture on ubuntu-latest, manually
    # triggered, to gather real variance evidence before committing to the
    # runner move. Never required by ci-gate. contents: read only, no
    # elevated permissions, no pull_request_target.
    frontend-perf-shadow-ubuntu:
      if: github.event_name == 'workflow_dispatch'
      runs-on: ubuntu-latest
      permissions:
        contents: read
      steps:
        - name: Check out candidate
          uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803 # v6
          with:
            persist-credentials: false
        - name: Install LLVM 22 (Linux, via apt.llvm.org)
          run: |
            wget https://apt.llvm.org/llvm.sh
            chmod +x llvm.sh
            sudo ./llvm.sh 22
            sudo apt-get install -y libpolly-22-dev
            echo "LLVM_SYS_221_PREFIX=/usr/lib/llvm-22" >> "$GITHUB_ENV"
        - name: Benchmark candidate, 5 replicates
          run: |
            set -euo pipefail
            for round in 1 2 3 4 5; do
              cargo bench --locked --bench check_bench -- --save-baseline "shadow-$round"
            done
        - name: Print replicate medians to job summary
          run: |
            set -euo pipefail
            {
              echo "### frontend-perf-shadow-ubuntu replicate medians"
              for round in 1 2 3 4 5; do
                jq -r '.median.point_estimate' \
                  "target/criterion/pycc_check_frontend_fixture/shadow-$round/estimates.json"
              done
            } >> "$GITHUB_STEP_SUMMARY"
  ```

  Add `workflow_dispatch:` to the workflow's top-level `on:` block if not already present (check first — other jobs like `hook-install-check.yml` may already use a *separate* workflow file for their own `workflow_dispatch` trigger; if `ci.yml`'s own `on:` block has no `workflow_dispatch` today, adding one here is a new top-level trigger and must be checked against `scripts/check_ci_permissions.rb`'s own policy before this task's commit, per AGENTS.md's CI permission-boundary rules — run `ruby scripts/check_ci_permissions.rb` after this change and fix anything it flags).

  This job is **not** covered by `check_roadmap_evidence.rb`'s `frontend-perf-measure`/`frontend-perf-gate` structural checks (different job name), so it needs no new Ruby constants — but confirm `ruby scripts/check_roadmap_evidence.rb .` still passes after adding it (a new, unrecognized job name should not trip any existing check; if it does, investigate before proceeding — do not silently work around a real trust-boundary concern the checker is correctly raising).

- [ ] **Step 2: Commit and push, open the Stage PR**

  ```bash
  git add .github/workflows/ci.yml
  git commit -m "ci: add temporary frontend-perf-shadow-ubuntu workflow_dispatch job for D-112 evidence-gathering"
  git push -u origin fix/issue-109-frontend-perf-gate-runner
  gh pr create --title "Stage D-112: ubuntu-latest frontend-perf-gate shadow measurement" --body "$(cat <<'EOF'
  ## Summary
  - Stages new `D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB`/`_GATE_JOB` constants in `scripts/check_roadmap_evidence.rb` (inactive — not yet in `REVIEWED_PERF_CI_WORKFLOW_SHA256S`).
  - Adds a temporary, non-required `frontend-perf-shadow-ubuntu` workflow_dispatch job to gather real ubuntu-latest variance evidence before any activation decision (issue #109).
  - Does not change the live `frontend-perf-measure`/`frontend-perf-gate` jobs.

  Part of issue #109's remedy (D-112, `docs/DECISIONS.md`). Refs #109.
  EOF
  )"
  ```

  Wait for CI green (this PR's own diff is Ruby/YAML-only; `frontend-perf-gate` itself still runs the OLD macos-14 shape against this PR and may still flake — that is expected and unrelated to this task; do not chase it here). Get the pinned reviewer's pass (Task 6 covers running it — for this intermediate PR, a lighter-weight self-review against Global Constraints is sufficient; reserve the full pinned-reviewer pass for the activation PR in Task 5, since that is where behavior actually changes). Merge once required checks are green.

---

### Task 4: Gather shadow-measurement evidence

**Files:**
- Modify: `docs/DECISIONS.md` (D-112's Consequences / a dated Update note)

**Interfaces:**
- Consumes: `frontend-perf-shadow-ubuntu`'s `$GITHUB_STEP_SUMMARY` output from Task 3.
- Produces: the accept/reject decision Task 5 is gated on.

- [ ] **Step 1: Trigger the shadow job several times on `main` post-merge**

  ```bash
  gh workflow run ci.yml --ref main -f dummy=1 2>&1 || gh workflow run ci.yml --ref main
  ```
  (Adjust to however `workflow_dispatch` inputs, if any, are actually declared once Task 3 lands — the job itself takes none, so a bare `gh workflow run ci.yml --ref main` should suffice; consult `gh workflow run --help` if the workflow requires selecting the specific job vs. the whole file.) Trigger **at least 5 separate runs** (matching this project's own established minimum sample size, e.g. D-062's replicated-measurement precedent) spread over as much wall-clock time as practical (different times of day reduces the odds of measuring one contended CI queue window as if it were representative).

- [ ] **Step 2: Collect and analyze the medians**

  For each run, `gh run view <run-id> --job <shadow-job-id> --log | grep -A10 "replicate medians"` (or read `$GITHUB_STEP_SUMMARY` via the API) to get the 5 replicate medians per run. Compute the coefficient of variation (stdev/mean) both *within* each run's 5 replicates and *across* runs' aggregate medians. Compare against this project's own already-recorded `macos-14` history from issue #109 (9-measurement record cited in D-109's own Correction note, `docs/DECISIONS.md`) and the job-duration CV table cited in D-112's Context.

- [ ] **Step 2: Record the finding as a dated Update note on D-112**

  Append to D-112 in `docs/DECISIONS.md` (do not edit the original Context/Decision text — matching this project's own append-only ADR-correction convention):
  ```markdown
    **Update (<date>): shadow-measurement evidence.** <N> runs of `frontend-perf-shadow-ubuntu` on `main` produced <CV%> coefficient of variation across replicate medians (raw data: <run URLs>). This <supports/does not support> activation. <If supporting:> Proceeding to Task 5. <If not:> Not proceeding — see the user-facing report for next steps; the `getrusage` alternative from this ADR's own Alternatives section remains the documented fallback.
  ```

  **If the evidence does not support activation**: STOP. Do not proceed to Task 5. Report this honestly — do not force activation on unfavorable evidence just to make progress; that would repeat exactly the mistake #109's own history already shows this project correctly avoiding (D-096/D-101's explicit rejection of "re-run until favorable"). Surface the finding and ask the user whether to pursue the `getrusage` alternative instead, accept the runner move's now-measured limitations, or take a different approach.

  Commit:
  ```bash
  git add docs/DECISIONS.md
  git commit -m "docs: record D-112 shadow-measurement evidence"
  git push
  ```

---

### Task 5: Activation (only if Task 4's evidence supports it)

**Files:**
- Modify: `.github/workflows/ci.yml` (flip `frontend-perf-measure`/`frontend-perf-gate` to the new shape; remove the Task 3 shadow job)
- Modify: `scripts/check_roadmap_evidence.rb` (flip `REVIEWED_PERF_CI_WORKFLOW_SHA256S`; retire `D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256` to historical)
- Modify: `scripts/test_check_roadmap_evidence.rb` (flip the "contains only active X" tests; add "D100 remains a reviewed audit fixture" test; update/rename the full activation test)
- Modify: `docs/DECISIONS.md` (D-112 Status → `accepted`, dated Update note with acceptance evidence)
- Modify: `docs/ROADMAP.md` if it references `frontend-perf-gate`'s runner anywhere (check first: `grep -n "macos-14\|frontend-perf" docs/ROADMAP.md`)

**Interfaces:**
- Consumes: `D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB`/`_GATE_JOB`/`_CI_WORKFLOW_SHA256` (Task 2).
- Produces: the new live CI shape every subsequent PR (including the now-unblocked PR-10/PR-11) will run against.

- [ ] **Step 1: Start a fresh branch off updated `main`** (Task 2/3's stage PR has merged by now — start clean per D-021)

- [ ] **Step 2: Flip `.github/workflows/ci.yml`**

  Replace the live `frontend-perf-measure`/`frontend-perf-gate` job bodies with exactly the shape in `tests/fixtures/d112-ubuntu-frontend-perf-ci.yml` (byte-for-byte — copy the two job bodies directly from the fixture file rather than retyping, to guarantee the whole-file digest matches). Remove the Task 3 shadow job entirely (its purpose is served).

- [ ] **Step 3: Recompute and verify the whole-file digest**

  ```bash
  ruby -rdigest -e 'puts Digest::SHA256.file(".github/workflows/ci.yml").hexdigest'
  ```
  This **must** equal `D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256` from Task 2 (since the shadow job's own removal changes the file's bytes relative to the original fixture-construction moment — if the fixture in Task 2 Step 3 was built including a shadow-job placeholder, the digest **will not match** here; if that's the case, rebuild `tests/fixtures/d112-ubuntu-frontend-perf-ci.yml` now to reflect the true final `ci.yml` — without the shadow job — recompute its digest, and update `D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256` accordingly in the same commit as this activation; the fixture is a record of "what the live file will look like," and Task 3's shadow job was always meant to be transient, so the fixture should never have included it — flag this explicitly to the task reviewer if Task 2 built the fixture from a pre-shadow-job state, since then no correction is needed here and this note is moot).

- [ ] **Step 4: Flip `REVIEWED_PERF_CI_WORKFLOW_SHA256S` and retire D100**

  ```ruby
  REVIEWED_PERF_CI_WORKFLOW_SHA256S = [D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256].freeze
  ```
  Change `D100_COMPOSE_D91_D99_CI_WORKFLOW_SHA256`'s comment to `# Historical audit-fixture digest. The public policy no longer accepts it.` (matching every other retired digest's exact comment text) — keep the constant defined, never delete it.

- [ ] **Step 5: Update tests**

  - Rename/rewrite `test_tier1_workflow_authorization_contains_only_active_d100` → asserts `[D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256] == REVIEWED_PERF_CI_WORKFLOW_SHA256S`.
  - Rename/rewrite `test_tier1_workflow_authorization_is_the_active_d100_digest`-equivalent → hashes the live file, asserts equality with `D112_UBUNTU_FRONTEND_PERF_CI_WORKFLOW_SHA256`.
  - Add a "D100 remains a reviewed audit fixture" test mirroring `test_d91_relax_frontend_perf_manifest_workflow_remains_an_audit_fixture`'s shape but for D100 (fixture — if one doesn't already exist for D100's own shape, check `tests/fixtures/d100-compose-d91-d99-ci.yml`, which the research already confirmed exists).
  - Update/replace `test_d112_ubuntu_frontend_perf_workflow_is_staged_not_active` (Task 2) — it now asserts the *opposite* (the digest IS in the active array); rename to `test_d112_ubuntu_frontend_perf_workflow_is_now_active` or fold into a comprehensive `test_d112_ubuntu_frontend_perf_workflow_is_active_and_reviewed` test modeled on `test_d100_composed_workflow_is_active_and_reviewed`.

- [ ] **Step 6: Run the full Ruby suite, then the whole-repo Rust suite** (this touches no Rust code, but confirm nothing else broke)

  ```bash
  LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/test_check_roadmap_evidence.rb
  LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb .
  ruby scripts/check_ci_permissions.rb
  ```

- [ ] **Step 7: Update D-112's Status and docs, commit, push, open the Activation PR**

  Update D-112: `Status: accepted`, append a dated Update note citing this activation's own commit/PR. Check and update `docs/ROADMAP.md`/`docs/AGENT_TOOLING.md`/`docs/TESTING.md` for any stale `macos-14`/`frontend-perf-gate` runner references (grep first, per AGENTS.md's "keep documentation current" rule — do not skip this by assuming there are none).

  ```bash
  git add -A
  git commit -m "ci: activate D-112, frontend-perf-measure/gate now runs on ubuntu-latest"
  git push -u origin <activation-branch-name>
  gh pr create --title "Activate D-112: frontend-perf-gate now runs on ubuntu-latest" --body "..."
  ```

  Run the pinned local reviewer (D-068, `ievo:deep-reviewer`) on this PR's full diff before merging — this is the PR that actually changes required-check behavior, so it gets the full review, unlike Task 3's stage PR.

---

### Task 6: Close-out

- [ ] **Step 1: Verify CI green on the activation PR**, including `frontend-perf-gate` itself passing on the new runner (this is the actual proof the fix works — do not merge on faith that it "should" pass).

- [ ] **Step 2: Verify CI green on the exact post-merge `main` commit** (D-078/D-066 discipline — re-fetch and re-check right before any docs claim "resolved").

- [ ] **Step 3: Close issue #109** with a comment citing the exact runs (mirroring every prior "acceptance evidence" comment's own format in that issue's history) — only if genuinely resolved; if residual variance is later observed (as happened repeatedly through D-051→D-056→D-062), reopen honestly rather than treating a single green run as proof, matching this issue's own hard-won lesson.

- [ ] **Step 4: Update `docs/SESSION_LOG.md`** per D-066 with this fix's outcome, and note that PR-10/PR-11 (and, transitively, PR-12/13/14) are now unblocked from the CI-gate side (a rebase against the now-advanced `main` is still separately required before either can actually merge — do not conflate "gate fixed" with "PR-10 ready to merge").
