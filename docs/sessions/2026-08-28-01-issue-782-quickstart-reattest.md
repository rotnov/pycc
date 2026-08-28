# Session handoff: issue #782 completion — quick_start.rs migration via evidence-hero re-attestation

## Status

The final PR of issue #782 (Part 2 of #779's scratch-directory migration),
implemented against `origin/main` at `c8176cc5` on branch
`feat/issue-782-quickstart-reattest`. This entry lands with the PR's merge
(D-192). **This PR completes and closes #782**: `tests/quick_start.rs`'s
single raw `std::env::temp_dir().join(...)` site — the one Batch D
(PR #817) carved out because the public site's versioned evidence-hero
contract pinned the file's exact bytes — is migrated onto
`pycc_scratch::ScratchDir`, and the landing hero is re-attested against
the migrated bytes in the same pull request.

## What this PR delivers

- Commit A (`8324332d`, the new evidence commit): the test's raw site
  migrated onto `ScratchDir` (named local spanning the test, `.expect()`,
  root `create_dir_all` dropped — Drop owns cleanup; test name, fixture,
  and snapshot untouched), and the `tests/quick_start.rs` entry removed
  from `scripts/check_scratch_dir_usage.py`'s ALLOWLIST together with the
  docstring/header exception clauses.
- Commit B: the re-attestation. `site/evidence-heroes.json` rotated to the
  new canonical-LF SHA-256 of the migrated test file
  (`cb7af27a…9199a766`), `repository.commit`/`stable_links` → commit A,
  `attestation.run_id`/`run_url` and the five Tier-1 platform `job_url`s →
  commit A's green CI run `33198103510`; the same constants rotated in
  `scripts/check-site.sh`'s `LANDING_ALLOWLIST`,
  `scripts/test-check-site.sh`, docs/WEBSITE.md's prose, and the three
  site projections (`site/index.html`, `site/index.html.md`,
  `site/llms.txt`). All replacements are length-identical (40-hex commit,
  64-hex digest, 11-digit run/job ids), so the D-200 llms.txt byte budget
  is unchanged.
- Attestation-run choice, recorded per D-127: the prior attestation was a
  `main`-push run, but rotating a pin through a main run needs two PRs
  with the site gate red on `main` between them. The validator is hermetic
  (it validates recorded constants and local git blobs, never calls
  GitHub), and the PR CI run of commit A executes the identical workflow
  across all five Tier-1 platform legs, so that run is recorded as the
  accepted attestation and every `main` commit stays green.
- Docs: `docs/TESTING.md`'s scratch-directories status paragraph flipped
  to the completed state; a dated 2026-08-28 addendum appended to D-201;
  no ROADMAP change (test-infrastructure migration, no behavior/milestone
  change, and the D-200 llms.txt headroom forbids ROADMAP growth anyway).

## Gates (all green at this snapshot, macOS local run)

- `python3 scripts/check_scratch_dir_usage.py` — PASS (ALLOWLIST is
  exactly `src/main.rs: 2`)
- `python3 -m unittest discover -s scripts -p 'test_check_scratch*'` — 12 OK
- `cargo test --test quick_start` — PASS on the migrated file
- `bash scripts/check-site.sh` — "Website checks passed." against the
  rotated identities
- `bash scripts/test-check-site.sh` — validator self-tests passed
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb` — PASS

## Pending — NOT delivered by this PR

- #783 (the two production sites in `src/main.rs` — note PR #823 touched
  `src/main.rs` since Batch B; re-verify the two sites' shape before
  planning), #784 (bounded stale scratch-root cleanup), #785 (operational
  TMPDIR guidance + closing verification) — untouched, tracked under the
  parent #779.

## Where to resume

#782 closes with this PR's merge; #779's remaining sequence is #783, then
#784, then #785. The `src/main.rs` ALLOWLIST entry in
`scripts/check_scratch_dir_usage.py` is the mechanical tracker: #783's PR
removes it, leaving the ALLOWLIST empty — the recorded completeness signal
for #779 Parts 2/3.
