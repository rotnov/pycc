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

pre-commit passes the selected filenames after the entry, producing one
`pycc check -- FILE...` invocation. The `--` boundary keeps leading-hyphen
filenames positional without consuming `-h` or `--help`, and paths remain in
the operating system's native representation through file access. pycc checks
every file so one early failure does not hide later diagnostics. Exit `0`
accepts the commit, `1` rejects it for compile diagnostics, and `2` rejects it
for invocation or input I/O errors. When both `1`-class and `2`-class failures
occur, `2` takes precedence.

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

## Release and verification

A revision advertised for hook use must satisfy all of the following:

1. `pre-commit validate-manifest .pre-commit-hooks.yaml` succeeds.
2. A clean pre-commit environment can install the Rust hook from that exact
   revision.
3. Multiple valid Python files pass in one invocation, including files with a
   supported Python source-encoding declaration.
4. Syntax errors, current-version capability errors, unreadable inputs, and
   mixed failures produce the documented diagnostics and exit codes.
5. The repository's normal build, tests, documentation, clippy, workflow
   policy, and 100% line/region coverage gates pass.

The checked-in integration tests enforce the manifest contents, execute the
hook's valid-source fixture through `pycc check`, and cover the CLI success,
failure, aggregation, and precedence paths. A release still needs the
clean-environment and Tier-1 installation evidence; merging the manifest alone
does not create or advertise a release tag.
