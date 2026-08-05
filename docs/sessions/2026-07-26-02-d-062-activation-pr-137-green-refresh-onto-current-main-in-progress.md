# 2026-07-26 — D-062 activation PR #137 green; refresh onto current main in progress

**Snapshot evidence:** draft [PR #137](https://github.com/rotnov/pycc/pull/137)
at head `b6f5a29d4c56d65d88d82120595bbc04343c6f25` was based on
`e433b849ef1083c0af7aa6da6c022a6e0661dc9f`. Its first complete CI run
`30204610811` and every required check passed, Codex reported no major
issues for that exact head, and the PR had no review threads. While the
run completed, PR #128 advanced `main` to
`841048ec37e20d85a5a0406778f9ec8b66224b04`; the activation worktree is
therefore integrating that exact default-branch commit before review and
merge. This containing snapshot includes the resolved documentation from
that integration but does not itself claim that PR #137 has merged.

**Performance-gate state:** PR #137 activates the staged D-062 workflow
byte-for-byte and retires D-056's live workflow digest while preserving
its identical-input telemetry rule. The first unselected 5+5 PR artifacts
are `frontend-perf-previous` ID `8632698165` and
`frontend-perf-current` ID `8632713088`, retained for 90 days. Their five
per-run medians aggregate to `6869.56 ns -> 6967.66 ns` (`+1.4281%`). The
exact base/head diff contains no `src/` or `crates/` change, so the trusted
classifier reports identical executable inputs; the comparator also falls
within 2% if evaluated as changed. This validates byte-exact execution and
fixed evidence handling, but does not exercise the blocking changed-input
path.

**Required next steps:** finish the non-force merge of current `main`,
rerun focused local policy/site checks, push the new head, run the review
retry checker before requesting Codex review for that SHA, and merge only
after the repeated required checks are green with no unresolved threads.
Then verify the post-merge `main` CI and history audit. Keep issue #109
open until repeated changed-source PR and post-merge runs validate D-062's
blocking aggregate without result selection. PR #132 is a likely future
changed-source observation only after it rebases; its unrelated draft
D-062 through D-065 ADR range currently collides with `main`, and the
renumbering finding is recorded on that PR. **Update from the entry
above:** by the time PR #137 merged (as `45545bb`), PR #132 had already
resolved that collision (D-057–061 kept, D-062 ceded to `main`, remaining
four entries moved to D-070–073) — this entry's last sentence is
superseded, kept verbatim below as the historical record it actually was
at the time it was written.
