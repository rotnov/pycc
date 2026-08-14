# The arena's run environment. One image for every harness; per-harness state
# (auth, caches) lives in a named volume mounted at /root, so nothing from the
# operator's machine reaches a run: no user settings, no globally installed
# skills, no packages preinstalled into the system python. All three leaked
# into host runs and were measured biasing them.
FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        git python3 python3-venv ca-certificates curl ripgrep procps \
    && rm -rf /var/lib/apt/lists/*

# uv into /usr/local/bin, NOT into /root: the home directory is shadowed by the
# per-harness volume at run time, and a binary living there would vanish.
ENV UV_INSTALL_DIR=/usr/local/bin
RUN curl -LsSf https://astral.sh/uv/install.sh | sh

# The harness CLIs.
RUN npm install -g @anthropic-ai/claude-code @openai/codex @xai-official/grok

# devin ships native binaries — cli.devin.ai carries aarch64/x86_64 linux
# bundles. Its installer resolves everything under $HOME, which the
# per-harness volume shadows at run time, so it installs under /opt/devin
# and only the launcher is linked into the PATH.
# bash, not sh: the installer uses `set -o pipefail`, which dash rejects.
# `|| true` because the installer ends by launching an interactive login,
# which a TTY-less build rightly cancels — exit 1 with the binary installed.
# The `test -x` right after is the check that actually matters.
RUN HOME=/opt/devin bash -c 'curl -fsSL https://cli.devin.ai/install.sh | bash' || true
RUN test -x /opt/devin/.local/bin/devin \
    && ln -sf /opt/devin/.local/bin/devin /usr/local/bin/devin

# Agents sometimes `git commit` inside a fixture; without an identity that is
# a stall, not a test result.
RUN git config --system user.email arena@localhost \
    && git config --system user.name arena \
    && git config --system init.defaultBranch main

ENV HOME=/root
WORKDIR /work
