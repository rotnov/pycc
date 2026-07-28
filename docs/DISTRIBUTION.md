# pycc Distribution

This document owns the distribution contract that exists today. pycc remains
pre-alpha: binary installers, signing, package-manager publication, and
`rustup`-style channels are not specified yet.

## Pre-commit hook

The main pycc repository is also the hook repository. Its
`.pre-commit-hooks.yaml` manifest publishes one hook:

| Field | Contract |
|---|---|
| id | `pycc-check` |
| entry | `pycc check --` |
| language | `rust` |
| inputs | staged files identified by pre-commit as Python |
| mutation | none |
| scheduling | one process at a time (`require_serial: true`) |

Consumers pin a release tag or immutable commit:

```yaml
repos:
  - repo: https://github.com/rotnov/pycc
    rev: <release-tag-or-commit>
    hooks:
      - id: pycc-check
```

pre-commit passes the selected filenames after the entry in one or more serial
`pycc check -- FILE...` batches. At most one hook process runs at a time;
pre-commit may split a large path set to respect platform command-line limits.
The `--` boundary keeps leading-hyphen filenames positional without consuming
`-h` or `--help`, and paths remain in the operating system's native
representation through file access. Within each batch, pycc checks every file
so one early failure does not hide later diagnostics. Exit `0` accepts the
commit, `1` rejects it for compile diagnostics, and `2` rejects it for
invocation or input I/O errors. When both `1`-class and `2`-class failures
occur in one batch, `2` takes precedence.

## Current installation boundary

`language: rust` makes pre-commit install the repository with Cargo into its
own cache. The root `pycc` binary currently has the LLVM-backed code generator
as a normal dependency, so this first installation requires the repository's
pinned Rust 1.97.1 and LLVM 22.1.1 toolchain. `pycc check` itself stops after
parsing, HIR lowering, and type checking and does not invoke LLVM or link a
runtime.

This is an explicit alpha limitation. The hook must not be described as a
lightweight or toolchain-free installation while the package graph still
builds codegen. Likewise, a tag must not be described as cross-platform until
the hook installation and pass/fail behavior are green on every Tier-1 target.

Building `pycc` from source needs more than an LLVM 22.1.1 install on `PATH`:

- `LLVM_SYS_221_PREFIX` must point at that LLVM 22 install; `llvm-sys` does
  not discover it on its own. `.github/workflows/ci.yml`'s per-platform setup
  steps are the canonical, tested way to obtain and export it for each
  Tier-1 target (Linux via `apt.llvm.org`, macOS via Homebrew, Windows via
  LLVM's own prebuilt release archive).
- On Windows, `pycc`'s own linker step shells out to `clang.exe` directly
  (D-028), outside any Developer Command Prompt, so a plain `cargo install`
  also needs that environment set up (`ilammy/msvc-dev-cmd` in CI). This
  LLVM release's own `llvm-config` reports libxml2 as a system lib (D-027),
  so `llvm-sys` additionally needs a real `xml2s.lib` on `LIB`; `ci.yml`
  installs it via `vcpkg`.

None of this is optional or hook-specific: it is what building `pycc` from
source requires on each platform, whether via `cargo build`, `cargo install`,
or pre-commit's own installer.

## Release and verification

A revision advertised for hook use must satisfy all of the following:

1. `pre-commit validate-manifest .pre-commit-hooks.yaml` succeeds.
2. A clean pre-commit environment can install the Rust hook from that exact
   revision.
3. One batch containing multiple valid Python files passes, including files
   with a supported Python source-encoding declaration.
4. Syntax errors, current-version capability errors, unreadable inputs, and
   mixed failures produce the documented diagnostics and exit codes.
5. The repository's normal build, tests, documentation, clippy, workflow
   policy, and 100% line/region coverage gates pass.

The checked-in integration tests enforce the manifest contents, execute the
hook's valid-source fixtures through `pycc check`, and cover the CLI success,
failure, aggregation, and precedence paths. Items 1-3 additionally need
pre-commit's own installer exercised end-to-end on every Tier-1 target, which
those integration tests do not drive.
`.github/workflows/hook-install-check.yml` is the canonical way to gather that
evidence: a `workflow_dispatch`-only job that reproduces `ci.yml`'s per-target
toolchain setup above, then runs `pre-commit validate-manifest` and
`pre-commit try-repo` against `tests/fixtures/pre_commit_valid.py` and
`tests/fixtures/pre_commit_encoding.py` on all five Tier-1 targets. Dispatch
it from the revision under consideration (`gh workflow run
hook-install-check.yml --ref <revision>`) and record the per-target outcome
here before advertising that revision for hook use. A release still needs
this dated Tier-1 evidence; merging the manifest, or this workflow itself,
does not by itself create or advertise a release tag.
