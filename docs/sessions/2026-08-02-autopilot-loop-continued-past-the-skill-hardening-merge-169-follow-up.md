# 2026-08-02 — Autopilot loop continued past the skill-hardening merge; #169 follow-up shipped, #243 set aside, a new D-103 gap folded back into the skills

**Authoritative checkpoint:** `origin/main`'s tip is `c498075` (PR #277,
"propose D-112 ci.yml successor"). This entry's own skill-fix work lives on
branch `claude/fold-d103-manifest-awareness`, based on that same commit, not
yet merged — see its own pull request for the exact commit range and
CI/review outcome.

**What happened, in order, continuing the standing autopilot directive after
the entry below's `claude/issue-skill-hardening` merged:**

1. **Issue #169** (reopened: `manage_ievo_hooks.py --root` accepted an
   ancestor symlink or mount point, not just a symlinked leaf) was
   re-selected, re-planned, and shipped as PR #274 — a new
   `ensure_cli_root_is_not_redirected` gate called from `main()` before
   `.resolve()`, walking every ancestor component of the raw `--root`
   argument. Recorded as D-113 (a new decision superseding D-081 in the
   D-077→D-081 style, not an in-place edit — `@codex review` flagged the
   first draft's in-place D-081 addendum as mutating accepted-decision
   history, and the repository's own D-077→D-081 precedent confirmed the
   fix). 402 tests (6 Windows-only skips). Merged clean.
2. **Issue #243** (`check_search_visibility_audit.py`'s missing
   subprocess/CLI-boundary test coverage, the same gap class `manage_ievo_hooks.py`
   already had a precedent for) was selected next and planned through 5
   adversarial review rounds. Round 5 found a defect serious enough to
   invalidate the plan's single-PR shape: `scripts/test_check_search_visibility_audit.py`
   — the exact file the plan edited — is itself a `tests/fixtures/policy-successor-manifest.json`
   (D-103) protected target. Independently reproduced twice (once
   masked by the pre-existing, unrelated D-112 `ci.yml` staging gap; once
   isolated from it) that a direct edit fails the required `audit` check
   with `"candidate protected policy target ... lacks a base-staged
   successor."` Consulted an independent advisor, who confirmed: fixing
   #243 correctly needs D-103's own two-PR stage-then-activate process, but
   the *activation* half of an already-in-flight, unrelated D-112 transition
   (PR #278, `ci.yml`) is stuck on a maintainer `emergency-bypass`
   authorization this session cannot grant — landing #243's own stage half
   on top would add a second concurrent staged-but-unactivated protected
   target, deepening a repository-wide `audit` block rather than curing
   anything. **No code was changed for #243 itself.** Per-issue stop
   condition; #243 is denylisted for the remainder of this run per
   `issue-select`'s `## Loop` section. No GitHub write was made on #243 —
   this log entry is the record for the next session instead.
3. **This entry's own fix:** neither `issue-select`'s blocker screen nor
   `issue-implement`'s staged-pattern trigger had ever checked
   `tests/fixtures/policy-successor-manifest.json` — both only knew about
   the narrower, `ci.yml`-specific D-080 digest-allowlist stage-then-activate
   mechanism (itself, on closer reading, not fully independent of D-103:
   `scripts/check_roadmap_evidence.rb` — the file D-080's own stage PR edits
   directly — is itself commonly a D-103 manifest entry, which is exactly why
   the real precedent needed a *prior*, separate stage/activate pair for that
   file, PRs #271/#273, before the later `ci.yml` pair, PRs #277/#278, could
   land — confirmed by reading both PRs' file lists directly, not assumed).
   Folded the general D-103 manifest check into both skills: each skill's
   step 1 baseline/preflight now checks the manifest for any mid-transition
   entry up front — this blocks every candidate PR's `audit` check
   repository-wide regardless of which issue or files are involved (verified
   directly against `scripts/check_ci_permissions.rb`'s
   `validate_policy_successor_transition`, which compares every manifest
   target unconditionally, not only ones the candidate PR's own diff
   touches), so it is now a **systemic** stop condition, not a per-issue one
   as first drafted — a D-068 pinned-reviewer round on this change caught
   the misclassification before merge. `issue-select` step 4 and
   `issue-implement` step 4
   separately still check whether an individual candidate's own likely
   fix touches a manifest path, as a per-issue deprioritization signal.

**Update (2026-08-02, same day): PR #278 merged** — see the entry directly
above this one for the full resolution (round 7a/7b, D-112 accepted). #243
itself is still not implemented — its own D-103 stage/activate work was
never started, per the per-issue stop condition above — but the *reason*
it was blocked (an unrelated manifest entry stuck mid-transition) no
longer holds as of this update; a future session should re-verify #243's
premise fresh rather than assume this stale account still applies.

**Aside, still generally true regardless of #278's resolution:** while this
block was live, PRs #258/#236 showed a stale `audit` **pass** even though a
fresh run would have failed — their last run predated PR #277's merge, and
`audit` triggers on `pull_request_target` only (`.github/workflows/workflow-policy.yml`),
which (per this file's 2026-07-30 D-100 entry) reuses an already-completed
run's resolved base-ref checkout on a bare `gh run rerun` rather than
re-resolving `main` fresh — only an actual new `synchronize` event forces
re-evaluation. Worth remembering next time any check's pass/fail history is
read across a base-branch change.
