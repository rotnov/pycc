## Protect main

- `main` accepts changes only through pull requests. Branch protection requires the current CI check, resolved conversations, and an up-to-date branch.
- Before any `git commit`, verify the current branch is not the protected
  default branch. If it is, create a feature branch from the current
  commit first and commit there. A local commit on the protected branch
  bypasses the PR-based review and CI gates that protect it, even when
  the commit is not pushed.
- While the repository has only one maintainer, require zero approving reviews so the PR path remains usable.
- Administrators and automation credentials do not bypass the rule for ordinary work.
