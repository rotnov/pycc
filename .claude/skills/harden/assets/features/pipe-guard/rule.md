- A status-bearing command (`git commit`, `docker build`, `pytest`, linters)
  is never piped into a filter — the pipeline's exit status is the filter's,
  not the command's. `.claude/hooks/pipe-guard.py` blocks the call before it
  runs; run the command bare and filter its saved output afterwards, or opt
  in explicitly with `set -o pipefail` / `PIPESTATUS`.
