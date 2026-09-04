---
id: D-227
title: "Partition the llms.txt context ceiling across per-resource budgets"
status: accepted
---

## D-227: Partition the llms.txt context ceiling across per-resource budgets
- Status: accepted
- Context: [#923](https://github.com/rotnov/pycc/issues/923). Issue #207's
  bounded-expansion contract gives `site/llms-txt-context-manifest.json` an
  aggregate ceiling (`budget_kib`) and a per-resource `budget_bytes` for each
  of the six non-optional documents. At `12650781` the six actual sizes summed
  to 278296 bytes against the 278528-byte ceiling
  ([D-218](./D-218-raise-the-llms-txt-aggregate-context-ceiling-to.md) raised it
  from 264 KiB to 272 KiB): **232 bytes** of headroom, less than a sixth of one
  merged issue's roadmap paragraph. Two facts made that worse than it looks.
  First, the six `budget_bytes` summed to **339968** against the 278528-byte
  ceiling -- oversubscribed by **61440** -- so no per-resource budget could ever
  fire before the aggregate one; they were decorative, and an over-budget
  failure named no document. Second, `docs/ROADMAP.md` alone was 187328 bytes,
  67% of the whole expansion, because its `Current delivery status` evidence
  cells and its per-pull-request follow-up paragraphs had been written
  append-only for months: every merged issue added narrative that was already
  recorded in `docs/decisions/`, `docs/sessions/`, or the same document's own
  milestone sections. The observed growth has two regimes -- a *suppressed*
  ~145 bytes per merge while the budget was nearly full (36 merges over 7 days,
  D-200 through D-218), and an *unsuppressed* 1400-1800 bytes per merged issue
  when it was not. The #910 paragraph was drafted at ~1800 bytes and cut to
  ~900 purely to fit; that forced cut is the harm this issue exists to remove.
- Decision: **recover the space editorially and make the per-resource budgets
  real, rather than raising the ceiling a third time.** Three parts.
  (1) Condense `docs/ROADMAP.md` by 46051 bytes (187328 → 141277), entirely by
  removing accumulated per-pull-request history from the eleven free-form
  `Current delivery status` evidence cells and from six historical follow-up
  paragraphs, keeping each cell's current claim, its primary evidence link, and
  its remaining gap. Nothing removed is the only record of itself: every
  removed item survives in the cited decision entry, in `docs/sessions/`, or in
  the milestone section that already describes it. The `Public evidence and
  discoverability` row was left byte-for-byte intact -- it is not narrative but
  a machine-bound projection whose exact phrases five Pages prose checkers
  require.
  (2) Rebalance all six `budget_bytes` so they **partition** the ceiling:
  13312 / 18432 / 10240 / 20480 / 43008 for the five stable documents and
  168960 for `docs/ROADMAP.md`, summing to 274432 and leaving 4096 bytes of the
  ceiling deliberately unallocated. `budget_kib` is unchanged at 272.
  (3) Enforce `sum(budget_bytes) <= budget_kib * 1024` in `scripts/check-site.sh`,
  placed after the aggregate check so the aggregate check stays reachable in
  the `budget_kib = 1` negative control.
- Alternatives:
  - **Raise `budget_kib` a third time (264 → 272 → 280+).** Rejected. The first
    two raises bought 8 KiB each and were consumed in days; a third would buy
    ~4 merged issues and re-arrive at the same place with a larger context that
    every llms.txt consumer pays for on every expansion. The ceiling is a
    published consumer contract, not a dial. This alternative was considered
    first and explicitly, and rejected on the measured growth rates above.
  - **Split `docs/ROADMAP.md` into per-milestone files.** Deferred, not
    rejected. It would work, but it crosses at least eight seams that machine
    checks bind (`check_roadmap_evidence.rb`'s checklist, the five Pages prose
    checkers' phrase set, `check_readme_milestone_projection.rb`'s section
    extraction, `check_conformance_breadth.py`'s headline parse,
    `check_status_page_freshness.rb`'s three-way signal, the manifest binding
    itself, `site/llms.txt`'s published link set, and every relative link into
    the file). Two of those fail *blind* rather than loudly: the freshness
    check's feature-paragraph set and the prose checkers' phrase set both go
    quiet if their input file simply stops existing at the bound path. That is
    a change that deserves its own issue and its own plan, not a rider on this
    one.
  - **Trim the other five documents instead.** Rejected: together they are
    90968 bytes, they are not the growth surface, and `docs/PYTHON_STANDARDS.md`
    grows by conformance rows that are themselves the evidence.
  - **Delete the aggregate check now that it is unreachable.** Rejected; see
    Consequences.
- Consequences:
  - The manifest's per-resource budgets are now enforceable allocations. An
    over-budget failure names the document responsible instead of reporting a
    total.
  - **Headroom is partitioned, not shared.** `docs/ROADMAP.md` can no longer
    draw on another document's unused slack, and vice versa. That is the point
    -- shared headroom is what let the roadmap grow to 67% of the expansion
    unnoticed -- but it means a document approaching its own budget must be
    condensed or explicitly rebudgeted in a reviewed change, even while the
    aggregate has room.
  - **The aggregate check is now provably unreachable** and is knowingly
    retained. If every actual is within its own budget and the budgets sum
    within the ceiling, the actuals sum within the ceiling. It stays because it
    is the direct statement of the ceiling `docs/WEBSITE.md` publishes to
    consumers, and because it keeps the ceiling enforced if this invariant is
    later weakened. `scripts/test-check-site.sh`'s three budget controls
    therefore assert on the rejecting check's own message, not on exit status
    alone: an exit-status-only assertion would let the `budget_kib = 1` fixture
    keep passing while the aggregate check silently stopped being exercised.
  - Runway: the condensation recovers 46051 bytes (187328 -> 141277); adding
    this change's own roadmap paragraph leaves `docs/ROADMAP.md` at 143311
    bytes, **25649 bytes of slack** inside its own 168960-byte budget --
    roughly 14 unsuppressed 1800-byte paragraphs, against a prior 232 bytes of
    shared headroom. Aggregate headroom is 44249 bytes, but the roadmap's own
    budget is now the binding constraint, which is the point: a failure names
    the document. The paragraph length that fits is no longer set by the
    budget.
  - `AGENTS.md` and `docs/WEBSITE.md` now carry the standing convention that
    made the regrowth possible in the first place: an evidence cell states the
    current claim, its primary link, and the remaining gap, and history goes to
    the decision log, `docs/sessions/`, or the milestone section. Without that
    convention this change buys time, not a fix.
  - This supersedes the operative part of
    [D-218](./D-218-raise-the-llms-txt-aggregate-context-ceiling-to.md), whose
    Consequences said the raise "does not touch any per-resource budget".
    D-218's ceiling stands; its hands-off stance on per-resource budgets does
    not.
