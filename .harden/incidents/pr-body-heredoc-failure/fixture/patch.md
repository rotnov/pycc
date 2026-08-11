## Repository governance

### Pull requests

- `main` accepts changes only through pull requests.
- When creating pull requests with `gh pr create`, always write the PR body
  to a temporary file and use `--body-file <path>`. Never inline a heredoc
  in the `--body` argument — nested quoting in a shell command call is not
  reliable and will fail on bodies containing apostrophes or backticks.
